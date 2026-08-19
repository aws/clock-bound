//! Amazon Time Sync IO Source

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info};

use super::ntp::{BurstConfig, Ntp, NtpIoError};
use super::{ClockDisruptionEvent, ControlRequest};
use crate::daemon::{async_ring_buffer, event, selected_clock::SelectedClockSource};

pub const BURST_DURATION: Duration = Duration::from_secs(1);
pub const BURST_INTERVAL: Duration = Duration::from_millis(50);

pub const ADDRESS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(169, 254, 169, 123), 123);
pub const INTERVAL_DURATION: Duration = Duration::from_secs(2);
pub const SAMPLE_TIMEOUT: Duration = Duration::from_millis(100);

/// Contains the data needed to run the Amazon Time Sync runner.
#[derive(Debug)]
pub struct AmazonTimeSync {
    ntp: Ntp,
}

impl AmazonTimeSync {
    /// Constructs a new `AmazonTimeSync` with using given parameters.
    ///
    /// NOTE:
    /// The `AmazonTimeSync` object will start in burst mode. The timer for burst mode begins when
    /// the object is constructed, NOT when the run loop begins.
    pub fn construct(
        event_sender: async_ring_buffer::Sender<event::Ntp>,
        ctrl_receiver: mpsc::Receiver<ControlRequest>,
        clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
        selected_clock: Arc<SelectedClockSource>,
    ) -> Self {
        let burst_config = BurstConfig {
            duration: BURST_DURATION,
            interval: BURST_INTERVAL,
        };
        let ntp = Ntp::builder()
            .event_sender(event_sender)
            .ctrl_receiver(ctrl_receiver)
            .clock_disruption_receiver(clock_disruption_receiver)
            .selected_clock(selected_clock)
            .extensions(vec![]) // No extensions for Amazon Time Sync
            .socket_address(ADDRESS.into())
            .timeout(SAMPLE_TIMEOUT)
            .interval(INTERVAL_DURATION)
            .burst_config(burst_config)
            .build();
        AmazonTimeSync { ntp }
    }

    /// NTP Amazon Time Sync task runner.
    ///
    /// Samples NTP packets from the AWS EC2 Amazon Time Sync address.
    ///
    /// The function runs in two modes a normal mode and a burst mode.
    ///
    /// While in burst mode the Amazon Time Sync source is polled more frequently
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
    #[tracing::instrument(level = "info", skip_all, fields(identifier = %self.ntp.socket_address()))]
    pub async fn run(&mut self) -> Result<(), NtpIoError> {
        debug!("Starting Amazon Time Sync sampling loop.");
        loop {
            let branches = self.ntp.select_branches();
            tokio::select! {
                biased; // priority order is disruption, commands, and ticks
                val = branches.clock_disruption_receiver.changed() => {
                    if let Err(e) = val {
                        error!(?e, "Clock disruption receiver dropped.");
                        break;
                    }
                    if self.ntp.handle_disruption() {
                        info!("Amazon Time Sync source transitioning from `Normal` mode to `Burst` mode.");
                    }
                }
                ctrl_req = branches.ctrl_receiver.recv() => {
                    match ctrl_req {
                        None => {
                            // this select can happen if `SourceIO` drops the ctrl_sender
                            break;
                        },
                        Some(ControlRequest::Shutdown) => {
                            debug!("Received shutdown signal. Exiting.");
                            break;
                        },
                    }
                }
                _ = branches.interval.tick() => {
                    self.handle_interval_tick().await;
                }
            }
        }
        debug!("Amazon Time Sync runner exiting.");
        Ok(())
    }

    // todo: this is prefactoring to give a place to allow for retrying with IPv6 vs IPv4
    async fn handle_interval_tick(&mut self) {
        let ntp_event = match self.ntp.sample().await {
            Ok(ntp_event) => ntp_event,
            Err(e) => {
                tracing::debug!(?e, "Failed to sample Amazon Time Sync source.");
                return;
            }
        };

        if let Err(e) = self.ntp.send_event(&ntp_event) {
            error!(?e, "channel closed. Not supported on this task");
            panic!("unable to communicate with daemon. {e:?}");
        }

        if self.ntp.handle_burst_expiry() {
            info!("Amazon Time Sync source transitioning from `Burst` mode to `Normal` mode.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_amazon_time_sync() -> (
        AmazonTimeSync,
        watch::Sender<ClockDisruptionEvent>,
        Arc<SelectedClockSource>,
    ) {
        let (event_sender, _) = async_ring_buffer::create::<event::Ntp>(1);
        let (_, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (clock_disruption_sender, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent {
                disruption_marker: None,
            });

        let selected_clock = Arc::new(SelectedClockSource::default());
        (
            AmazonTimeSync::construct(
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
        let (mut amazon_time_sync, clock_disruption_sender, _) = create_amazon_time_sync().await;

        // Send a disruption signal
        clock_disruption_sender
            .send(ClockDisruptionEvent {
                disruption_marker: Some(1),
            })
            .unwrap();

        amazon_time_sync.ntp.handle_disruption();

        let branches = amazon_time_sync.ntp.select_branches();
        assert_eq!(branches.interval.period(), BURST_INTERVAL,);
    }

    #[tokio::test(start_paused = true)]
    async fn validate_to_normal_mode() {
        let (mut amazon_time_sync, clock_disruption_sender, _) = create_amazon_time_sync().await;

        // Send a disruption signal to enter burst mode
        clock_disruption_sender
            .send(ClockDisruptionEvent {
                disruption_marker: Some(1),
            })
            .unwrap();

        amazon_time_sync.ntp.handle_disruption();

        // Verify we're in burst mode
        let branches = amazon_time_sync.ntp.select_branches();
        assert_eq!(branches.interval.period(), BURST_INTERVAL);

        // Advance time past the burst duration to trigger expiry
        tokio::time::advance(BURST_DURATION + Duration::from_millis(1)).await;

        // handle_burst_expiry should detect elapsed burst and transition to normal
        assert!(amazon_time_sync.ntp.handle_burst_expiry());

        // Verify the interval is now the normal interval
        let branches = amazon_time_sync.ntp.select_branches();
        assert_eq!(branches.interval.period(), INTERVAL_DURATION);
    }
}
