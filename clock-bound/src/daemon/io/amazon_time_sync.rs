//! Amazon Time Sync IO Source

use std::{num::Wrapping, sync::Arc};
use thiserror::Error;
use tokio::{
    io,
    net::UdpSocket,
    sync::{mpsc, watch},
    time::{self, Instant, Interval, MissedTickBehavior, interval, interval_at},
};
use tracing::{debug, info};

use super::ntp::{
    AMAZON_TIME_SYNC_ADDRESS, AMAZON_TIME_SYNC_BURST_DURATION,
    AMAZON_TIME_SYNC_BURST_INTERVAL_DURATION, AMAZON_TIME_SYNC_INTERVAL_DURATION,
    AMAZON_TIME_SYNC_TIMEOUT, packet,
};
use super::{ClockDisruptionEvent, ControlRequest};
use crate::daemon::{
    async_ring_buffer::{self, SendError},
    event::{self},
    io::ntp::{self, SamplePacketError, packet::Timestamp},
    selected_clock::SelectedClockSource,
};

use packet::Packet;

#[derive(Debug, Error)]
pub enum AmazonTimeSyncError {
    #[error("IO failure.")]
    Io(#[from] io::Error),
    #[error("Failed to parse NTP packet.")]
    PacketParsing(String),
    #[error("Mismatched origin. Expected {expected}, got {received}")]
    OriginMismatch { expected: u64, received: u64 },
    #[error("Operation timed out.")]
    Timeout(#[from] time::error::Elapsed),
    #[error("TSC order failure. tsc_pre: {pre}. tsc_post: {post}")]
    TscOrder { pre: u64, post: u64 },
    #[error("IO failure on socket clear")]
    SocketClear(#[source] io::Error),
}

impl From<SamplePacketError> for AmazonTimeSyncError {
    fn from(value: SamplePacketError) -> Self {
        match value {
            SamplePacketError::Io(e) => AmazonTimeSyncError::Io(e),
            SamplePacketError::Timeout(e) => AmazonTimeSyncError::Timeout(e),
            SamplePacketError::TscOrder { pre, post } => {
                AmazonTimeSyncError::TscOrder { pre, post }
            }
            SamplePacketError::SocketClear(e) => AmazonTimeSyncError::SocketClear(e),
        }
    }
}

/// Contains the data needed to run the Amazon Time Sync runner.
#[derive(Debug)]
pub struct AmazonTimeSync {
    socket: UdpSocket,
    event_sender: async_ring_buffer::Sender<event::Ntp>,
    ctrl_receiver: mpsc::Receiver<ControlRequest>,
    clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
    ntp_buffer: [u8; Packet::MIN_SIZE],
    interval: Interval,
    mode: Mode,
    selected_clock: Arc<SelectedClockSource>,
    transmit_counter: Wrapping<u64>,
}

impl AmazonTimeSync {
    /// Constructs a new `AmazonTimeSync` with using given parameters.
    ///
    /// NOTE:
    /// The `AmazonTimeSync` object will start in burst mode. The timer for burst mode begins when the object is constructed,
    /// NOT when the run loop begins.
    pub fn construct(
        socket: UdpSocket,
        event_sender: async_ring_buffer::Sender<event::Ntp>,
        ctrl_receiver: mpsc::Receiver<ControlRequest>,
        clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
        selected_clock: Arc<SelectedClockSource>,
    ) -> Self {
        let mut ats_interval = interval(AMAZON_TIME_SYNC_BURST_INTERVAL_DURATION);
        ats_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        AmazonTimeSync {
            socket,
            event_sender,
            ctrl_receiver,
            clock_disruption_receiver,
            ntp_buffer: [0u8; Packet::MIN_SIZE],
            interval: ats_interval,
            mode: Mode::burst(),
            selected_clock,
            transmit_counter: Wrapping(0),
        }
    }

    /// Samples the Amazon Time Sync source.
    ///
    /// When sampling from a NTP source we first collect the current time stamp counter
    /// value. We then send a NTP request and await for a response. Once we receive a
    /// response we again collect the current time stamp counter value. After we've
    /// collected the NTP sample we construct the `Event` and push that event through
    /// to the ring buffer.
    async fn sample(&mut self) -> Result<event::Ntp, AmazonTimeSyncError> {
        let counter = self.transmit_counter.0;
        self.transmit_counter += 1;
        let (source, stratum) = self.selected_clock.get_with_client_stratum();
        let packet = Packet::builder()
            .transmit_timestamp(Timestamp::new(counter))
            .stratum(stratum.into())
            .reference_id(source.into())
            .build();
        packet.emit_bytes(&mut self.ntp_buffer);

        let ntp_event = ntp::sample_packet(
            &self.socket,
            AMAZON_TIME_SYNC_ADDRESS.into(),
            &mut self.ntp_buffer,
            AMAZON_TIME_SYNC_TIMEOUT,
            counter,
        )
        .await?;
        debug!(?ntp_event, "Received packet.");
        Ok(ntp_event)
    }

    /// NTP Amazon Time Sync task runner.
    ///
    /// Samples NTP packets from the AWS EC2 Amazon Time Sync address.
    ///
    /// The function runs in two modes a normal mode and a burst mode.
    ///
    /// While in burst mode the Amazon Time Sync source is polled more frequently,
    /// [`AMAZON_TIME_SYNC_BURST_INTERVAL_DURATION`].
    /// Burst mode is triggered when:
    /// - a clock disruption signal is received.
    /// - ...
    ///
    /// Burst mode is active for a set amount of time, [`AMAZON_TIME_SYNC_BURST_DURATION`], before
    /// transitioning back to normal mode.
    ///
    /// # Panics
    /// Function will panic if not called within the `tokio` runtime.
    ///
    /// # Errors
    /// Returns error if loop exits unexpectedly
    pub async fn run(&mut self) -> Result<(), AmazonTimeSyncError> {
        // Sampling loop
        info!("Starting Amazon Time Sync sampling loop.");
        loop {
            tokio::select! {
                biased; // priority order is disruption, commands, and ticks
                val = self.clock_disruption_receiver.changed() => {
                    if let Err(e) = val {
                        tracing::error!(?e, "Clock disruption receiver dropped.");
                        break;
                    }
                    // Clock Disruption logic here
                    self.handle_disruption();
                    info!("Received clock disruption signal. Entering Burst mode.");
                }
                ctrl_req = self.ctrl_receiver.recv() => {
                    // Ctrl logic here.
                    match ctrl_req {
                        None => {
                            // this select can happen if `SourceIO` drops the ctrl_sender
                            break;
                        },
                        Some(ControlRequest::Shutdown) => {
                            info!("Received shutdown signal. Exiting.");
                            break;
                        },
                    }
                }
                _ = self.interval.tick() => {
                    self.handle_interval_tick().await;
                }


            }
        }
        info!("Amazon Time Sync runner exiting.");
        Ok(())
    }

    async fn handle_interval_tick(&mut self) {
        let ntp_event = match self.sample().await {
            Ok(ntp_event) => ntp_event,
            Err(e) => {
                debug!(?e, "Failed to sample Amazon Time Sync source.");
                return;
            }
        };

        match self.event_sender.send(ntp_event.clone()) {
            Ok(()) => debug!(?ntp_event, "Successfully sent Amazon Time Sync IO event."),
            Err(SendError::Disrupted(_)) => {
                tracing::info!("Trying to send when there was a disruption event.");
            }
            Err(SendError::BufferClosed(e)) => {
                tracing::error!(?e, "Amazon Time Sync channel closed not supported in alpha");
                panic!("Amazon Time Sync unable to communicate with daemon. {e:?}");
            }
        }

        if let Mode::Burst(start_time) = self.mode
            && start_time.elapsed() >= AMAZON_TIME_SYNC_BURST_DURATION
        {
            self.transition_to_normal_mode();
            info!("Transitioning from `Burst` mode to `Normal` mode.");
        }
    }

    /// Handles a clock disruption event
    ///
    /// Changes the source's mode to [`Mode::Burst`] and the polling frequency to
    /// [`AMAZON_TIME_SYNC_BURST_INTERVAL_DURATION`].
    fn handle_disruption(&mut self) {
        let AmazonTimeSync {
            socket: _socket,
            event_sender,
            ctrl_receiver: _ctrl_receiver,
            clock_disruption_receiver,
            ntp_buffer: _ntp_buffer,
            interval: ats_interval,
            mode,
            selected_clock: _selected_clock,
            transmit_counter: _,
        } = self;

        let val = clock_disruption_receiver.borrow_and_update().clone();
        if val.disruption_marker.is_some() {
            event_sender.handle_disruption();
            *mode = Mode::burst();
            *ats_interval = interval(AMAZON_TIME_SYNC_BURST_INTERVAL_DURATION);
            ats_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            ats_interval.reset_immediately();
        }
    }

    /// Changes the source's mode to [`Mode::Normal`] and changes polling frequency to
    /// [`AMAZON_TIME_SYNC_INTERVAL_DURATION`].
    fn transition_to_normal_mode(&mut self) {
        let AmazonTimeSync {
            socket: _socket,
            event_sender: _event_sender,
            ctrl_receiver: _ctrl_receiver,
            clock_disruption_receiver: _clock_disruption_receiver,
            ntp_buffer: _ntp_buffer,
            interval: ats_interval,
            mode,
            selected_clock: _selected_clock,
            transmit_counter: _,
        } = self;

        *mode = Mode::Normal;
        // `tokio::time::interval` first tick completes immediately,
        // which would cause us to immediately handle that tick, and poll the NTP server again.
        // To avoid that behavior, we set the interval to start ahead by at least `AMAZON_TIME_SYNC_INTERVAL_DURATION`.
        *ats_interval = interval_at(
            Instant::now() + AMAZON_TIME_SYNC_INTERVAL_DURATION,
            AMAZON_TIME_SYNC_INTERVAL_DURATION,
        );
        ats_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
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

    async fn create_amazon_time_sync() -> (
        AmazonTimeSync,
        watch::Sender<ClockDisruptionEvent>,
        Arc<SelectedClockSource>,
    ) {
        let (event_sender, _) = async_ring_buffer::create::<event::Ntp>(1);
        let socket = UdpSocket::bind(ntp::UNSPECIFIED_SOCKET_ADDRESS)
            .await
            .unwrap();
        let (_, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (clock_disruption_sender, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent {
                disruption_marker: None,
            });

        let selected_clock = Arc::new(SelectedClockSource::default());
        (
            AmazonTimeSync::construct(
                socket,
                event_sender,
                ctrl_receiver,
                clock_disruption_receiver,
                selected_clock.clone(),
            ),
            clock_disruption_sender,
            selected_clock,
        )
    }

    #[tokio::test]
    async fn validate_to_burst_mode() {
        let (mut amazon_time_sync, _, _) = create_amazon_time_sync().await;
        amazon_time_sync.handle_disruption();

        assert!(matches!(amazon_time_sync.mode, Mode::Burst(_)));
        assert_eq!(
            amazon_time_sync.interval.period(),
            AMAZON_TIME_SYNC_BURST_INTERVAL_DURATION
        );
    }

    #[tokio::test]
    async fn validate_to_normal_mode() {
        let (mut amazon_time_sync, _, _) = create_amazon_time_sync().await;
        amazon_time_sync.handle_disruption();
        amazon_time_sync.transition_to_normal_mode();

        assert!(matches!(amazon_time_sync.mode, Mode::Normal));
        assert_eq!(
            amazon_time_sync.interval.period(),
            AMAZON_TIME_SYNC_INTERVAL_DURATION
        );
    }
}
