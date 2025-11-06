//! Link Local IO Source

use thiserror::Error;
use tokio::{
    io,
    net::UdpSocket,
    sync::{mpsc, watch},
    time::{self, Duration, Instant, Interval, MissedTickBehavior, interval, timeout},
};
use tracing::{debug, info};

use super::ntp::{
    LINK_LOCAL_ADDRESS, LINK_LOCAL_BURST_DURATION, LINK_LOCAL_INTERVAL_DURATION,
    LINK_LOCAL_TIMEOUT, packet,
};
use super::tsc::read_timestamp_counter;
use super::{ClockDisruptionEvent, ControlRequest};
use crate::daemon::{
    async_ring_buffer,
    event::{self, NtpData},
    time::tsc::TscCount,
};

use packet::Packet;

/// The amount of time between source polls when in burst mode.
const BURST_INTERVAL_DURATION: Duration = Duration::from_millis(50);

#[derive(Debug, Error)]
pub enum LinkLocalError {
    #[error("IO failure.")]
    Io(#[from] io::Error),
    #[error("Failed to parse NTP packet.")]
    PacketParsing(String),
    #[error("Send NtpEvent message failed.")]
    SendEventMessage(#[from] mpsc::error::SendError<event::Ntp>),
    #[error("Operation timed out.")]
    Timeout(#[from] time::error::Elapsed),
    #[error("TSC order failure. tsc_pre: {pre}. tsc_post: {post}")]
    TscOrder { pre: u64, post: u64 },
}

/// Contains the data needed to run the link local runner.
#[derive(Debug)]
pub struct LinkLocal {
    socket: UdpSocket,
    event_sender: async_ring_buffer::Sender<event::Ntp>,
    ctrl_receiver: mpsc::Receiver<ControlRequest>,
    clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
    ntp_buffer: [u8; Packet::SIZE],
    interval: Interval,
    mode: Mode,
}

impl LinkLocal {
    /// Constructs a new `LinkLocal` with using given parameters.
    pub fn construct(
        socket: UdpSocket,
        event_sender: async_ring_buffer::Sender<event::Ntp>,
        ctrl_receiver: mpsc::Receiver<ControlRequest>,
        clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
    ) -> Self {
        let mut link_local_interval = interval(LINK_LOCAL_INTERVAL_DURATION);
        link_local_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        LinkLocal {
            socket,
            event_sender,
            ctrl_receiver,
            clock_disruption_receiver,
            ntp_buffer: [0u8; Packet::SIZE],
            interval: link_local_interval,
            mode: Mode::Normal,
        }
    }

    /// Samples the Link local source.
    ///
    /// When sampling from a NTP source we first collect the current time stamp counter
    /// value. We then send a NTP request and await for a response. Once we receive a
    /// response we again collect the current time stamp counter value. After we've
    /// collected the NTP sample we construct the `Event` and push that event through
    /// to the ring buffer.
    async fn sample(&mut self) -> Result<event::Ntp, LinkLocalError> {
        let packet = Packet::new_request(0);
        packet.emit_bytes(&mut self.ntp_buffer);

        // TODO: tsc reads and ntp samples need to be fenced.
        // We are currently investigating how to implement this appropriately.
        let sent_timestamp = read_timestamp_counter();

        // Request and Receive NTP sample.
        let recv_packet_result = timeout(LINK_LOCAL_TIMEOUT, {
            self.socket
                .send_to(&self.ntp_buffer, LINK_LOCAL_ADDRESS)
                .await?;
            self.socket.recv_from(&mut self.ntp_buffer)
        })
        .await?;

        let received_timestamp = read_timestamp_counter();

        #[cfg(all(not(test), feature = "test-side-by-side"))]
        let system_clock_reading = crate::daemon::event::SystemClockMeasurement::now();

        let (_, ntp_packet) = Packet::parse_from_bytes(&self.ntp_buffer)
            .map_err(|e| LinkLocalError::PacketParsing(e.to_string()))?;

        let ntp_data = NtpData::try_from(ntp_packet)
            .map_err(|e| LinkLocalError::PacketParsing(e.to_string()))?;

        let builder = event::Ntp::builder()
            .tsc_pre(TscCount::new(sent_timestamp.into()))
            .tsc_post(TscCount::new(received_timestamp.into()))
            .ntp_data(ntp_data);

        let ntp_event = {
            #[cfg(all(not(test), feature = "test-side-by-side"))]
            {
                builder.system_clock(system_clock_reading).build()
            }
            #[cfg(any(test, not(feature = "test-side-by-side")))]
            {
                builder.build()
            }
        };

        let ntp_event = ntp_event.ok_or(LinkLocalError::TscOrder {
            pre: sent_timestamp,
            post: received_timestamp,
        })?;

        debug!(?recv_packet_result, "Received packet.");
        Ok(ntp_event)
    }

    /// NTP Link Local task runner.
    ///
    /// Samples NTP packets from the AWS EC2 internal Link Local address.
    ///
    /// The function runs in two modes a normal mode and a burst mode.
    ///
    /// While in burst mode the link local source is polled more frequently,
    /// [`LINK_LOCAL_BURST_INTERVAL_DURATION`].
    /// Burst mode is triggered when:
    /// - a clock disruption signal is received.
    /// - ...
    ///
    /// Burst mode is active for a set amount of time, [`LINK_LOCAL_BURST_DURATION`], before
    /// transitioning back to normal mode.
    ///
    /// # Panics
    /// Function will panic if not called within the `tokio` runtime.
    ///
    /// # Errors
    /// Returns error if loop exits unexpectedly
    pub async fn run(&mut self) -> Result<(), LinkLocalError> {
        // Sampling loop
        info!("Starting link local sampling loop.");
        loop {
            tokio::select! {
                _ = self.interval.tick() => {
                    match self.sample().await {
                        Err(e) => {debug!(?e, "Failed to sample link local source.");}
                        Ok(ntp_event) => {
                            self.event_sender.send(ntp_event.clone()).expect("Buffer Closing is not expected in alpha.");
                            debug!(?ntp_event, "Successfully sent Link Local IO event.");
                        }
                    }
                    if let Mode::Burst(start_time) =  self.mode
                        && start_time.elapsed() >= LINK_LOCAL_BURST_DURATION {
                        self.transition_to_normal_mode();
                        info!("Transitioning from `Burst` mode to `Normal` mode.");
                    }
                }
                _ = self.ctrl_receiver.recv() => {
                    // Ctrl logic here.
                    // Currently we breakout of the loop if we receive a control event.
                    break;
                }
                _ = self.clock_disruption_receiver.changed() => {
                    // Clock Disruption logic here
                    self.transition_to_burst_mode();
                    info!("Received clock disruption signal. Entering Burst mode.");
                }
            }
        }
        info!("Link local runner exiting.");
        Ok(())
    }

    /// Changes the source's mode to [`Mode::Burst`] and the polling frequency to
    /// [`BURST_INTERVAL_DURATION`].
    fn transition_to_burst_mode(&mut self) {
        let LinkLocal {
            socket: _socket,
            event_sender: _event_sender,
            ctrl_receiver: _ctrl_receiver,
            clock_disruption_receiver: _clock_disruption_receiver,
            ntp_buffer: _ntp_buffer,
            interval: ll_interval,
            mode,
        } = self;

        *mode = Mode::burst();
        *ll_interval = interval(BURST_INTERVAL_DURATION);
        ll_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        ll_interval.reset_immediately();
    }

    /// Changes the source's mode to [`Mode::Normal`] and changes polling frequency to
    /// [`LINK_LOCAL_INTERVAL_DURATION`].
    fn transition_to_normal_mode(&mut self) {
        let LinkLocal {
            socket: _socket,
            event_sender: _event_sender,
            ctrl_receiver: _ctrl_receiver,
            clock_disruption_receiver: _clock_disruption_receiver,
            ntp_buffer: _ntp_buffer,
            interval: ll_interval,
            mode,
        } = self;

        *mode = Mode::Normal;
        *ll_interval = interval(LINK_LOCAL_INTERVAL_DURATION);
        ll_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    }
}

// TODO: Move to a higher level. This enum should be shared by all io sources.
/// An enum indicating the interval state of the of the source io.
///
/// # Variants:
/// - `Normal` mode indicates that the source is sampling at a constant frequency and remain so unless given an external signal.
///
/// - `Burst` mode indicates that the source is in a temporary mode during which the underlying source is polled more frequently.
#[derive(Debug)]
enum Mode {
    /// Indicates that the source is in its normal operating mode.
    Normal,
    /// Indicates that the source should be in burst mode and when it entered burst mode.
    Burst(Instant),
}

impl Mode {
    /// Constructs the [`Mode::Burst`] enum variant coupling the start time generation.
    fn burst() -> Mode {
        Mode::Burst(Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::io::ntp;

    async fn create_link_local() -> (LinkLocal, watch::Sender<ClockDisruptionEvent>) {
        let (event_sender, _) = async_ring_buffer::create::<event::Ntp>(1);
        let socket = UdpSocket::bind(ntp::UNSPECIFIED_SOCKET_ADDRESS)
            .await
            .unwrap();
        let (_, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (clock_disruption_sender, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent {
                disruption_marker: None,
            });

        (
            LinkLocal::construct(
                socket,
                event_sender,
                ctrl_receiver,
                clock_disruption_receiver,
            ),
            clock_disruption_sender,
        )
    }

    #[tokio::test]
    async fn validate_to_burst_mode() {
        let (mut link_local, _) = create_link_local().await;
        link_local.transition_to_burst_mode();

        assert!(matches!(link_local.mode, Mode::Burst(_)));
        assert_eq!(link_local.interval.period(), BURST_INTERVAL_DURATION);
    }

    #[tokio::test]
    async fn validate_to_normal_mode() {
        let (mut link_local, _) = create_link_local().await;
        link_local.transition_to_burst_mode();
        link_local.transition_to_normal_mode();

        assert!(matches!(link_local.mode, Mode::Normal));
        assert_eq!(link_local.interval.period(), LINK_LOCAL_INTERVAL_DURATION);
    }
}
