//! IP Address NTP IO Source
//!
//! Handles NTP sampling from fixed IP address sources (as opposed to DNS-resolved pools).

use std::{net::SocketAddr, sync::Arc};
use tokio::{
    sync::{mpsc, watch},
    time::Duration,
};
use tracing::{debug, error};

use super::ntp::{DaemonInfo, ExtensionField, Ntp, NtpIoError};
use super::{ClockDisruptionEvent, ControlRequest};
use crate::daemon::{async_ring_buffer, event, selected_clock::SelectedClockSource};

/// The amount of time between source polls in normal mode.
pub const INTERVAL: Duration = Duration::from_secs(16);
/// Per-sample timeout duration.
pub const SAMPLE_TIMEOUT: Duration = Duration::from_millis(100);

/// Tuple to hold both the `SocketAddr` and ring buffer `Sender` for an IO `IpAddrSource`
pub type Sender = (SocketAddr, async_ring_buffer::Sender<event::Ntp>);
/// Tuple to hold both the `SocketAddr` and ring buffer `Receiver` for an IO `IpAddrSource`
pub type Receiver = (SocketAddr, async_ring_buffer::Receiver<event::Ntp>);

/// Contains data used to run the IP address-based NTP source runner.
///
/// This source handles fixed IP addresses associated with NTP hosts.
/// It composes the base [`Ntp`] struct for all common IO plumbing.
///
/// Optionally supports burst mode when a [`BurstConfig`] is provided at construction.
/// Burst handling is fully encapsulated within the [`Ntp`] base struct.
#[derive(Debug)]
pub struct IpAddrSource {
    ntp: Ntp,
}

impl IpAddrSource {
    /// Constructs a new `IpAddrSource` with the given parameters.
    pub fn construct(
        address: SocketAddr,
        event_sender: async_ring_buffer::Sender<event::Ntp>,
        ctrl_receiver: mpsc::Receiver<ControlRequest>,
        clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
        selected_clock: Arc<SelectedClockSource>,
        daemon_info: DaemonInfo,
    ) -> Self {
        let extensions = vec![ExtensionField::Fec2V1(daemon_info)];
        let ntp = Ntp::builder()
            .event_sender(event_sender)
            .ctrl_receiver(ctrl_receiver)
            .clock_disruption_receiver(clock_disruption_receiver)
            .selected_clock(selected_clock)
            .extensions(extensions)
            .socket_address(address)
            .timeout(SAMPLE_TIMEOUT)
            .interval(INTERVAL)
            .build();
        IpAddrSource { ntp }
    }

    /// `IpAddrSource` task runner.
    ///
    /// Samples NTP packets from the IP address defined at initialization.
    ///
    /// If burst mode was configured at construction, burst/normal transitions are handled
    /// internally by the base [`Ntp`] struct.
    ///
    /// # Panics
    /// Function will panic if not called within the `tokio` runtime.
    ///
    /// # Errors
    /// Returns error if loop exits unexpectedly
    #[tracing::instrument(level = "info", skip_all, fields(identifier = %self.ntp.socket_address()))]
    pub async fn run(&mut self) -> Result<(), NtpIoError> {
        debug!("Starting IP addr source sampling loop.");
        loop {
            let branches = self.ntp.select_branches();
            tokio::select! {
                biased; // priority order is disruption, commands, and ticks
                val = branches.clock_disruption_receiver.changed() => {
                    if let Err(e) = val {
                        error!(?e, "Clock disruption receiver dropped.");
                        break;
                    }
                    debug!("Received clock disruption signal.");
                    self.ntp.handle_disruption();
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
        debug!("IP addr source runner exiting.");
        Ok(())
    }

    async fn handle_interval_tick(&mut self) {
        let ntp_event = match self.ntp.sample().await {
            Err(e) => {
                tracing::debug!(?e, "Failed to sample IP addr source.");
                return;
            }
            Ok(ntp_event) => ntp_event,
        };
        if let Err(e) = self.ntp.send_event(&ntp_event) {
            error!(?e, "channel closed. Not supported on this task");
            panic!("unable to communicate with daemon. {e:?}");
        }
    }
}
