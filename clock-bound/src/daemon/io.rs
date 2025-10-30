//! Perform IO on clock events
//!
//! This module implements the logic needed to retrieve time sync sample measurements, be it from NTP sources from
//! over the internet, the PHC via Linux's `ioctl` interface or some other source.

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};
use tokio::task::spawn;
use tracing::{debug, info, warn};

pub mod ntp;
use crate::daemon::{async_ring_buffer, event};

mod link_local;
use link_local::LinkLocal;

mod ntp_source;
use ntp_source::NTPSource;

pub mod tsc;

mod vmclock;

/// `SourceIO` acts as the front end for IO tasks.
///
/// `SourceIO` contains the interface from which new IO tasks can be spawned, as well as an interface
/// to send control commands to the specific IO tasks.
pub struct SourceIO {
    /// A mapping between the time source type and the task handle.
    sources: Sources,
    /// Contains the channel used to communicate clock disruption events.
    clock_disruption_channels: ClockDisruptionChannels,
}

impl SourceIO {
    /// Constructs a new `SourceIO` object and constructs the necessary resources.
    pub fn construct() -> Self {
        let (sender, receiver) = watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent {});
        SourceIO {
            sources: Sources::default(),
            clock_disruption_channels: ClockDisruptionChannels { sender, receiver },
        }
    }

    /// Spawns the IO task for sampling the Link Local NTP source.
    ///
    /// # Panics
    /// - If not called within the `tokio` runtime.
    /// - If socket binding fails.
    pub async fn create_link_local(&mut self, event_sender: async_ring_buffer::Sender<event::Ntp>) {
        info!("Creating link local source.");

        debug!(?self.sources.link_local, "Current source entry status");
        if self.sources.link_local.is_none() {
            self.sources.link_local = {
                let (ctrl_sender, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
                let clock_disruption_receiver = self.clock_disruption_channels.sender.subscribe();

                let socket = UdpSocket::bind(ntp::UNSPECIFIED_SOCKET_ADDRESS)
                    .await
                    .unwrap();

                let link_local = LinkLocal::construct(
                    socket,
                    event_sender,
                    ctrl_receiver,
                    clock_disruption_receiver,
                );
                Some(Source {
                    state: SourceState::Initialized(link_local),
                    ctrl_sender,
                })
            };
        }

        info!("Source update complete.");
    }

    /// Spawns the IO task for sampling a specific NTP Server source.
    ///
    /// # Panics
    /// - If not called within the `tokio` runtime.
    /// - If socket binding fails.
    pub async fn create_ntp_source(
        &mut self,
        server_address: SocketAddr,
        event_sender: async_ring_buffer::Sender<event::Ntp>,
    ) {
        info!(
            "Creating IO source from ntp server at {:#?}.",
            server_address.ip().to_string()
        );

        if !self.sources.ntp_sources.contains_key(&server_address) {
            let (ctrl_sender, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
            let clock_disruption_receiver = self.clock_disruption_channels.sender.subscribe();

            let socket = UdpSocket::bind(ntp::UNSPECIFIED_SOCKET_ADDRESS)
                .await
                .unwrap();

            let ntp_source = NTPSource::construct(
                socket,
                server_address,
                event_sender,
                ctrl_receiver,
                clock_disruption_receiver,
            );

            let source = Source {
                state: SourceState::Initialized(ntp_source),
                ctrl_sender,
            };
            self.sources.ntp_sources.insert(server_address, source);
        }

        info!("Source update complete.");
    }

    /// Starts the control flow task.
    #[allow(
        clippy::unused_async,
        reason = "This is a stubbed function. The async component will be implemented at a later date."
    )]
    pub async fn run(&self) {}

    /// Spawns all io tasks which have been initialized.
    ///
    /// This function will spawn all initialized io tasks. If a task is not initialized it will be
    /// skipped. Spawning will occur in an arbitrary order.
    pub fn spawn_all(&mut self) {
        // Spawn link local source
        if let Some(Source {
            state,
            ctrl_sender: _,
        }) = &mut self.sources.link_local
        {
            debug!("Attempting to spawn link local source.");
            if let SourceState::Initialized(mut link_local) = state.transition_to_running() {
                spawn(async move { link_local.run().await });
                debug!("Successfully spawned link local source.");
            } else {
                warn!("Attempted to spawn a link local source when one is currently running.");
            }
        } else {
            debug!("Could not spawn a link local source. No source data provided.");
        }

        // Spawn ntp sources
        for (key, ntp_source) in &mut self.sources.ntp_sources {
            debug!("Attempting to spawn {key:?} ntp source.");
            if let SourceState::Initialized(mut ntp_source) =
                ntp_source.state.transition_to_running()
            {
                spawn(async move { ntp_source.run().await });
                debug!("Successfully spawned ntp source.");
            } else {
                warn!("Attempted to spawn a ntp source when on is currently running.");
            }
        }
    }
}

/// Communication channels for sending and receiving clock disruption events.
struct ClockDisruptionChannels {
    sender: watch::Sender<ClockDisruptionEvent>,
    receiver: watch::Receiver<ClockDisruptionEvent>,
}

// TODO: This is a stub for future clock disruption events.
#[derive(Clone, Debug)]
pub struct ClockDisruptionEvent {}

// TODO: This is a stub for future control events.
#[derive(Debug)]
pub struct ControlRequest {}

/// A helper struct packaging the source state and its control sender together.
#[derive(Debug)]
struct Source<T> {
    state: SourceState<T>,
    ctrl_sender: mpsc::Sender<ControlRequest>,
}

/// `Sources` is a struct that maps sources to their state and control channels.
#[derive(Default, Debug)]
struct Sources {
    link_local: Option<Source<LinkLocal>>,
    ntp_sources: HashMap<SocketAddr, Source<NTPSource>>,
}

/// The possible states a time source can be in.
#[derive(Debug)]
pub enum SourceState<T> {
    Initialized(T),
    Running,
}

impl<T> SourceState<T> {
    fn is_initialized(&self) -> bool {
        matches!(self, SourceState::Initialized(_))
    }

    fn is_running(&self) -> bool {
        matches!(self, SourceState::Running)
    }

    /// Changes the state to `Running` and returns the previous state.
    fn transition_to_running(&mut self) -> SourceState<T> {
        std::mem::replace(self, SourceState::Running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn source_state_is_initialized() {
        let (event_sender, _) = async_ring_buffer::create::<event::Ntp>(1);
        let (_, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (_, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent {});

        let socket = UdpSocket::bind(ntp::UNSPECIFIED_SOCKET_ADDRESS)
            .await
            .unwrap();

        let link_local = LinkLocal::construct(
            socket,
            event_sender,
            ctrl_receiver,
            clock_disruption_receiver,
        );
        let current_state = SourceState::Initialized(link_local);
        assert!(current_state.is_initialized())
    }

    #[test]
    fn source_state_is_running() {
        let current_state = SourceState::<LinkLocal>::Running;
        assert!(current_state.is_running())
    }

    #[test]
    fn source_state_is_transitions() {
        let mut current_state = SourceState::<LinkLocal>::Running;
        assert!(current_state.transition_to_running().is_running())
    }

    #[tokio::test]
    async fn source_io_verify_link_local_creation() {
        let (event_sender, _) = async_ring_buffer::create::<event::Ntp>(1);

        let mut source_io = SourceIO::construct();
        source_io.create_link_local(event_sender).await;

        assert!(source_io.sources.link_local.is_some())
    }
}
