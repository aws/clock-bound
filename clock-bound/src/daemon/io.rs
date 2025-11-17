//! Perform IO on clock events
//!
//! This module implements the logic needed to retrieve time sync sample measurements, be it from NTP sources from
//! over the internet, the PHC via Linux's `ioctl` interface or some other source.

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};
use tokio_util::task::TaskTracker;
use tracing::{debug, info, warn};

pub mod ntp;
use crate::daemon::io::ntp::DaemonInfo;
use crate::daemon::selected_clock::SelectedClockSource;
use crate::daemon::{async_ring_buffer, event};

mod imds;

mod link_local;
use link_local::LinkLocal;

mod ntp_source;
use ntp_source::NTPSource;

mod phc;
use phc::Phc;

pub mod tsc;

mod vmclock;
use vmclock::VMClock;

/// `SourceIO` acts as the front end for IO tasks.
///
/// `SourceIO` contains the interface from which new IO tasks can be spawned, as well as an interface
/// to send control commands to the specific IO tasks.
pub struct SourceIO {
    /// The link local source.
    link_local: Option<Source<LinkLocal>>,
    /// Mapping between the socket ip-address and the ntp io source
    ntp_sources: HashMap<SocketAddr, Source<NTPSource>>,
    /// The PHC source.
    phc: Option<Source<Phc>>,
    /// The VMClock source
    vmclock: Option<Source<VMClock>>,
    /// Contains the channel used to communicate clock disruption events.
    clock_disruption_channels: ClockDisruptionChannels,
    /// Shared reference to the current selected clock source
    selected_clock: Arc<SelectedClockSource>,
    /// Daemon metadata
    daemon_info: DaemonInfo,
    /// `tokio::task::TaskTracker` to manage lifecycle of the individual IO tasks
    task_tracker: TaskTracker,
}

impl SourceIO {
    /// Constructs a new `SourceIO` object and constructs the necessary resources.
    pub fn construct(selected_clock: Arc<SelectedClockSource>, daemon_info: DaemonInfo) -> Self {
        let (sender, receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent::default());

        // Task tracker is closed on initialization. We only open it during calls to spawn
        // the IO tasks. This ensures that we don't introduce a deadlock if the shutdown function
        // is called before the spawn function.
        let task_tracker = TaskTracker::new();
        task_tracker.close();

        SourceIO {
            link_local: None,
            ntp_sources: HashMap::new(),
            phc: None,
            vmclock: None,
            clock_disruption_channels: ClockDisruptionChannels { sender, receiver },
            selected_clock,
            daemon_info,
            task_tracker,
        }
    }

    /// Initializes the IO task for sampling the Link Local NTP source.
    ///
    /// # Panics
    /// - If not called within the `tokio` runtime.
    /// - If socket binding fails.
    pub async fn create_link_local(&mut self, event_sender: async_ring_buffer::Sender<event::Ntp>) {
        info!("Creating link local source.");

        debug!(?self.link_local, "Current source entry status");
        if self.link_local.is_none() {
            self.link_local = {
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
                    self.selected_clock.clone(),
                );
                Some(Source {
                    state: SourceState::Initialized(link_local),
                    ctrl_sender,
                })
            };
        }

        info!("Source link local update complete.");
    }

    /// Initializes the IO task for sampling a specific NTP Server source.
    ///
    /// # Panics
    /// - If not called within the `tokio` runtime.
    /// - If socket binding fails.
    pub async fn create_ntp_source(&mut self, source: ntp::NTPSourceSender) {
        let (server_address, event_sender) = source;
        info!(
            "Creating IO source from ntp server at {:#?}.",
            server_address.ip().to_string()
        );

        if !self.ntp_sources.contains_key(&server_address) {
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
                self.selected_clock.clone(),
                self.daemon_info.clone(),
            );

            let source = Source {
                state: SourceState::Initialized(ntp_source),
                ctrl_sender,
            };
            self.ntp_sources.insert(server_address, source);
        }

        info!("Source NTP update complete.");
    }

    /// Initializes the IO task for sampling the PHC source.
    ///
    /// # Panics
    /// - If not called within the `tokio` runtime.
    pub async fn create_phc(&mut self, event_sender: async_ring_buffer::Sender<event::Phc>) {
        info!("Creating PHC source.");

        debug!(?self.phc, "Current PHC source entry status");
        if self.phc.is_none() {
            self.phc = {
                let (ctrl_sender, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
                let clock_disruption_receiver = self.clock_disruption_channels.sender.subscribe();
                match Phc::construct(event_sender, ctrl_receiver, clock_disruption_receiver).await {
                    Ok(phc) => Some(Source {
                        state: SourceState::Initialized(phc),
                        ctrl_sender,
                    }),
                    Err(e) => {
                        warn!("{}", e);
                        None
                    }
                }
            };
        }

        info!("Source PHC update complete.");
    }

    /// Returns true if a PHC source exists (has been created).
    pub fn phc_exists(&self) -> bool {
        self.phc.is_some()
    }

    /// Initializes the IO task for sampling the VMClock shared memory file.
    ///
    /// # Errors
    /// - If the vmclock shared memory file could not be found.
    /// - If the vmclock shared memory file is malformed.
    pub async fn create_vmclock(
        &mut self,
        vmclock_shm_path: &str,
    ) -> Result<(), vmclock::VMClockConstructionError> {
        info!("Creating link local source.");

        debug!(?self.vmclock, "Current source entry status.");
        if self.vmclock.is_none() {
            self.vmclock = {
                let (ctrl_sender, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);

                let vmclock = VMClock::construct(
                    vmclock_shm_path,
                    ctrl_receiver,
                    self.clock_disruption_channels.sender.clone(),
                )
                .await?;

                let source = Source {
                    state: SourceState::Initialized(vmclock),
                    ctrl_sender,
                };
                Some(source)
            };
        }
        Ok(())
    }

    // Creates a new [`watch::Receiver`] connected to the clock distribution watch [`watch::Sender`].
    pub fn clock_disruption_receiver(&self) -> watch::Receiver<ClockDisruptionEvent> {
        self.clock_disruption_channels.sender.subscribe()
    }

    /// Spawns all io tasks which have been initialized.
    ///
    /// This function will spawn all initialized io tasks. If a task is not initialized it will be
    /// skipped. Spawning will occur in an arbitrary order.
    pub fn spawn_all(&mut self) {
        self.task_tracker.reopen();
        // Spawn link local source
        if let Some(Source {
            state,
            ctrl_sender: _,
        }) = &mut self.link_local
        {
            debug!("Attempting to spawn link local source.");
            if let SourceState::Initialized(mut link_local) = state.transition_to_running() {
                self.task_tracker
                    .spawn(async move { link_local.run().await });
                debug!("Successfully spawned link local source.");
            } else {
                warn!("Attempted to spawn a link local source when one is currently running.");
            }
        } else {
            debug!("Could not spawn a link local source. No source data provided.");
        }

        // Spawn PHC source
        if let Some(Source {
            state,
            ctrl_sender: _,
        }) = &mut self.phc
        {
            debug!("Attempting to spawn PHC source.");
            if let SourceState::Initialized(mut phc) = state.transition_to_running() {
                self.task_tracker.spawn(async move { phc.run().await });
                debug!("Successfully spawned PHC source.");
            } else {
                warn!("Attempted to spawn a PHC source when one is currently running.");
            }
        } else {
            debug!("Could not spawn a PHC source. No source data provided.");
        }

        // Spawn vmclock source
        if let Some(Source {
            state,
            ctrl_sender: _,
        }) = &mut self.vmclock
        {
            if let SourceState::Initialized(mut vmclock) = state.transition_to_running() {
                self.task_tracker.spawn(async move { vmclock.run().await });
            } else {
                warn!("Attempted to spawn a vmclock source when one is currently running.");
            }
        } else {
            debug!("Could not spawn a vmclock source. No source data provided.");
        }

        // Spawn ntp sources
        for (key, ntp_source) in &mut self.ntp_sources {
            debug!("Attempting to spawn {key:?} ntp source.");
            if let SourceState::Initialized(mut ntp_source) =
                ntp_source.state.transition_to_running()
            {
                self.task_tracker
                    .spawn(async move { ntp_source.run().await });
                debug!("Successfully spawned ntp source.");
            } else {
                warn!("Attempted to spawn a ntp source when on is currently running.");
            }
        }

        self.task_tracker.close();
    }

    /// Sends shutdown signals to all io tasks.
    ///
    /// This function sequentially sends `ControlRequest::Shutdown` signals to all
    /// io tasks and then waits for all io tasks to exit.
    pub async fn shutdown_all(&mut self) {
        info!("Starting shutdown of SourceIO components.");
        // Shutdown link local source
        if let Some(Source {
            state: _,
            ctrl_sender,
        }) = &mut self.link_local
        {
            match ctrl_sender.send(ControlRequest::Shutdown).await {
                Ok(()) => info!("Successfully sent shutdown signal to link local source."),
                Err(e) => warn!(?e, "Failed to send shutdown signal to link local source."),
            }
        }

        // Shutdown vmclock source
        if let Some(Source {
            state: _,
            ctrl_sender,
        }) = &mut self.vmclock
        {
            match ctrl_sender.send(ControlRequest::Shutdown).await {
                Ok(()) => info!("Successfully sent shutdown signal to vmclock source."),
                Err(e) => warn!(?e, "Failed to send shutdown signal to vmclock source."),
            }
        }

        // Shutdown ntp sources
        for ntp_source in self.ntp_sources.values_mut() {
            match ntp_source.ctrl_sender.send(ControlRequest::Shutdown).await {
                Ok(()) => info!("Successfully sent shutdown signal to ntp source."),
                Err(e) => warn!(?e, "Failed to send shutdown signal to ntp source."),
            }
        }

        // Wait for all io tasks to exit
        info!("Waiting for {} io tasks to exit.", self.task_tracker.len());
        self.task_tracker.wait().await;
        info!("All tasks exited. Shutdown of io complete.");
    }
}

/// Communication channels for sending and receiving clock disruption events.
struct ClockDisruptionChannels {
    sender: watch::Sender<ClockDisruptionEvent>,
    receiver: watch::Receiver<ClockDisruptionEvent>,
}

#[derive(Clone, Debug, Default)]
pub struct ClockDisruptionEvent {
    pub disruption_marker: Option<u64>,
}

#[derive(Debug)]
pub enum ControlRequest {
    Shutdown,
}

/// A helper struct packaging the source state and its control sender together.
#[derive(Debug)]
struct Source<T> {
    state: SourceState<T>,
    ctrl_sender: mpsc::Sender<ControlRequest>,
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
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent::default());

        let socket = UdpSocket::bind(ntp::UNSPECIFIED_SOCKET_ADDRESS)
            .await
            .unwrap();

        let link_local = LinkLocal::construct(
            socket,
            event_sender,
            ctrl_receiver,
            clock_disruption_receiver,
            Arc::new(SelectedClockSource::default()),
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

        let info = DaemonInfo {
            major_version: 2,
            minor_version: 100,
            startup_id: 0xABCD_BCDE_CDEF_DEFA,
        };

        let mut source_io = SourceIO::construct(Arc::new(SelectedClockSource::default()), info);
        source_io.create_link_local(event_sender).await;

        assert!(source_io.link_local.is_some())
    }
}
