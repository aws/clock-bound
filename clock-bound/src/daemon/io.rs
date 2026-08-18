//! Perform IO on clock events
//!
//! This module implements the logic needed to retrieve time sync sample measurements, be it from NTP sources from
//! over the internet, the PHC via Linux's `ioctl` interface or some other source.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tokio_util::task::TaskTracker;
use tracing::{debug, info, warn};

pub mod ntp;
use crate::daemon::io::dns::Pool;
use crate::daemon::io::ntp::DaemonInfo;
use crate::daemon::selected_clock::SelectedClockSource;
use crate::daemon::{self, async_ring_buffer, event};

pub mod amazon_time_sync;
use amazon_time_sync::AmazonTimeSync;

pub mod dns;

pub mod in_use_ip_addrs;
use in_use_ip_addrs::InUseIpAddrs;

pub mod ip_addr_source;
use ip_addr_source::IpAddrSource;

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
    /// The Amazon Time Sync source.
    amazon_time_sync: Option<Source<AmazonTimeSync>>,
    /// Mapping between the socket ip-address and the ntp io source
    ip_addr_sources: HashMap<SocketAddr, Source<IpAddrSource>>,
    /// The PHC source.
    phc: Option<Source<Phc>>,
    /// Mapping of pool domain to the pool struct
    dns_pools: HashMap<String, Pool>,
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
    /// DNS message sender
    ///
    /// Passed into resolver structs to construct and delete NTP sources
    _dns_message_tx: mpsc::Sender<daemon::DnsMessage>,
    /// Shared tracker of IP addresses currently in use by NTP IO tasks
    in_use_ip_addrs: InUseIpAddrs,
}

impl SourceIO {
    /// Constructs a new `SourceIO` object and constructs the necessary resources.
    pub fn construct(
        selected_clock: Arc<SelectedClockSource>,
        daemon_info: DaemonInfo,
        dns_message_tx: mpsc::Sender<daemon::DnsMessage>,
    ) -> Self {
        let (sender, _) = watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent::default());

        // Task tracker is closed on initialization. We only open it during calls to spawn
        // the IO tasks. This ensures that we don't introduce a deadlock if the shutdown function
        // is called before the spawn function.
        let task_tracker = TaskTracker::new();
        task_tracker.close();

        SourceIO {
            amazon_time_sync: None,
            ip_addr_sources: HashMap::new(),
            dns_pools: HashMap::new(),
            phc: None,
            vmclock: None,
            clock_disruption_channels: ClockDisruptionChannels { sender },
            selected_clock,
            daemon_info,
            task_tracker,
            _dns_message_tx: dns_message_tx,
            in_use_ip_addrs: InUseIpAddrs::new(),
        }
    }

    /// Initializes the IO task for sampling the Amazon Time Sync NTP source.
    ///
    /// # Panics
    /// - If not called within the `tokio` runtime.
    pub fn create_amazon_time_sync(&mut self, event_sender: async_ring_buffer::Sender<event::Ntp>) {
        info!("Creating Amazon Time Sync source.");

        debug!(?self.amazon_time_sync, "Current source entry status");
        if self.amazon_time_sync.is_none() {
            self.amazon_time_sync = {
                let (ctrl_sender, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
                let clock_disruption_receiver = self.clock_disruption_channels.sender.subscribe();

                let amazon_time_sync = AmazonTimeSync::construct(
                    event_sender,
                    ctrl_receiver,
                    clock_disruption_receiver,
                    self.selected_clock.clone(),
                );
                Some(Source {
                    state: SourceState::Initialized(amazon_time_sync),
                    ctrl_sender,
                })
            };
        }

        info!("Source Amazon Time Sync update complete.");
    }

    /// Initializes the IO task for sampling a specific NTP Server source.
    ///
    /// # Panics
    /// - If not called within the `tokio` runtime.
    /// - If a duplicate IP is added (global to the app)
    pub fn create_ip_addr_source(&mut self, source: ip_addr_source::Sender) {
        let (server_address, event_sender) = source;
        info!(
            "Creating IO source from ntp server at {:#?}.",
            server_address.ip().to_string()
        );

        if !self.ip_addr_sources.contains_key(&server_address) {
            let (ctrl_sender, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
            let clock_disruption_receiver = self.clock_disruption_channels.sender.subscribe();

            let ip_addr_source = IpAddrSource::construct(
                server_address,
                event_sender,
                ctrl_receiver,
                clock_disruption_receiver,
                self.selected_clock.clone(),
                self.daemon_info.clone(),
            );

            let source = Source {
                state: SourceState::Initialized(ip_addr_source),
                ctrl_sender,
            };
            self.ip_addr_sources.insert(server_address, source);
            self.in_use_ip_addrs.add(server_address.ip()).unwrap();
        }

        info!("Source NTP update complete.");
    }

    /// Initializes an NTP pool
    #[tracing::instrument(level = "info", skip(self))]
    pub fn create_pool(&mut self, pool_domain: String) {
        info!("Creating DnsPool IO");

        if self.dns_pools.contains_key(&pool_domain) {
            info!("DnsPool IO already exists");
            return;
        }

        let dns_pool = Pool::new(pool_domain.clone());
        self.dns_pools.insert(pool_domain, dns_pool);

        info!("DnsPool IO constructed");
    }

    /// Pools getter
    pub fn pools(&self) -> &HashMap<String, Pool> {
        &self.dns_pools
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

    pub fn phc(&self) -> Option<&Phc> {
        self.phc.as_ref().and_then(|s| match &s.state {
            SourceState::Initialized(phc) => Some(phc),
            SourceState::Running => None,
        })
    }

    /// Returns true if a PHC source exists (has been created).
    pub fn phc_exists(&self) -> bool {
        self.phc.is_some()
    }

    /// Initializes the IO task for sampling the VMClock shared memory file.
    ///
    /// The caller is responsible for ensuring this is only called on supported instance types
    /// This method does not perform any platform checks.
    ///
    /// # Panics
    /// - When the vmclock device cannot be constructed using the provided shared memory path.
    pub async fn create_vmclock(&mut self, vmclock_shm_path: &str) {
        info!("Creating VMClock source.");

        info!(?self.vmclock, "Current source entry status.");
        if self.vmclock.is_none() {
            self.vmclock = {
                let (ctrl_sender, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);

                info!("Enabling VMClock.");
                let vmclock = VMClock::construct(
                    vmclock_shm_path,
                    ctrl_receiver,
                    self.clock_disruption_channels.sender.clone(),
                )
                .await
                .unwrap_or_else(|_| panic!("vmclock device not found {vmclock_shm_path}"));

                let source = Source {
                    state: SourceState::Initialized(vmclock),
                    ctrl_sender,
                };
                Some(source)
            };
        }
    }

    /// Returns `VMCLock` if it has been initialized
    pub fn vmclock(&self) -> Option<&VMClock> {
        self.vmclock
            .as_ref()
            .and_then(|source| match &source.state {
                SourceState::Initialized(vmclock) => Some(vmclock),
                SourceState::Running => None,
            })
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
        // Spawn Amazon Time Sync source
        if let Some(Source {
            state,
            ctrl_sender: _,
        }) = &mut self.amazon_time_sync
        {
            debug!("Attempting to spawn Amazon Time Sync source.");
            if let SourceState::Initialized(mut amazon_time_sync) = state.transition_to_running() {
                self.task_tracker
                    .spawn(async move { amazon_time_sync.run().await });
                debug!("Successfully spawned Amazon Time Sync source.");
            } else {
                warn!(
                    "Attempted to spawn a Amazon Time Sync source when one is currently running."
                );
            }
        } else {
            debug!("Could not spawn a Amazon Time Sync source. No source data provided.");
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

        // Spawn ip addr sources
        for (key, ip_addr_source) in &mut self.ip_addr_sources {
            debug!("Attempting to spawn {key:?} ip addr source.");
            if let SourceState::Initialized(mut ip_addr_source) =
                ip_addr_source.state.transition_to_running()
            {
                self.task_tracker
                    .spawn(async move { ip_addr_source.run().await });
                debug!("Successfully spawned ip addr source.");
            } else {
                warn!("Attempted to spawn an ip addr source when one is currently running.");
            }
        }

        #[expect(clippy::never_loop, reason = "DNS pool spawning not yet implemented")]
        for key in self.dns_pools.keys() {
            debug!("Attempting to spawn {key} dns pool.");
            unimplemented!("requires resolver impl to spawn");
        }

        self.task_tracker.close();
    }

    /// Sends shutdown signals to all io tasks.
    ///
    /// This function sequentially sends `ControlRequest::Shutdown` signals to all
    /// io tasks and then waits for all io tasks to exit.
    pub async fn shutdown_all(&mut self) {
        info!("Starting shutdown of SourceIO components.");
        // Shutdown Amazon Time Sync source
        if let Some(Source {
            state: _,
            ctrl_sender,
        }) = &mut self.amazon_time_sync
        {
            match ctrl_sender.send(ControlRequest::Shutdown).await {
                Ok(()) => info!("Successfully sent shutdown signal to Amazon Time Sync source."),
                Err(e) => warn!(
                    ?e,
                    "Failed to send shutdown signal to Amazon Time Sync source."
                ),
            }
        }

        // Shutdown PHC source
        if let Some(Source {
            state: _,
            ctrl_sender,
        }) = &mut self.phc
        {
            match ctrl_sender.send(ControlRequest::Shutdown).await {
                Ok(()) => info!("Successfully sent shutdown signal to phc source."),
                Err(e) => warn!(?e, "Failed to send shutdown signal to phc source."),
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

        // Shutdown ip addr sources
        for ip_addr_source in self.ip_addr_sources.values_mut() {
            match ip_addr_source
                .ctrl_sender
                .send(ControlRequest::Shutdown)
                .await
            {
                Ok(()) => info!("Successfully sent shutdown signal to ip addr source."),
                Err(e) => warn!(?e, "Failed to send shutdown signal to ip addr source."),
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
    /// Returns true if the state is `Initialized`.
    pub fn is_initialized(&self) -> bool {
        matches!(self, SourceState::Initialized(_))
    }

    /// Returns true if the state is `Running`.
    pub fn is_running(&self) -> bool {
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

        let amazon_time_sync = AmazonTimeSync::construct(
            event_sender,
            ctrl_receiver,
            clock_disruption_receiver,
            Arc::new(SelectedClockSource::default()),
        );
        let current_state = SourceState::Initialized(amazon_time_sync);
        assert!(current_state.is_initialized())
    }

    #[test]
    fn source_state_is_running() {
        let current_state = SourceState::<AmazonTimeSync>::Running;
        assert!(current_state.is_running())
    }

    #[test]
    fn source_state_is_transitions() {
        let mut current_state = SourceState::<AmazonTimeSync>::Running;
        assert!(current_state.transition_to_running().is_running())
    }

    #[tokio::test]
    async fn source_io_verify_amazon_time_sync_creation() {
        let (event_sender, _) = async_ring_buffer::create::<event::Ntp>(1);

        let info = DaemonInfo {
            major_version: 2,
            minor_version: 100,
            startup_id: 0xABCD_BCDE_CDEF_DEFA,
        };

        let (dns_message_tx, _dns_message_rx) = mpsc::channel::<daemon::DnsMessage>(1);
        let mut source_io = SourceIO::construct(
            Arc::new(SelectedClockSource::default()),
            info,
            dns_message_tx,
        );
        source_io.create_amazon_time_sync(event_sender);

        assert!(source_io.amazon_time_sync.is_some())
    }

    fn make_source_io() -> SourceIO {
        let info = DaemonInfo {
            major_version: 2,
            minor_version: 100,
            startup_id: 0xABCD_BCDE_CDEF_DEFA,
        };
        let (dns_message_tx, _) = mpsc::channel::<daemon::DnsMessage>(1);
        SourceIO::construct(
            Arc::new(SelectedClockSource::default()),
            info,
            dns_message_tx,
        )
    }

    #[test]
    fn create_pool_adds_pool() {
        let mut source_io = make_source_io();

        assert!(source_io.pools().is_empty());
        source_io.create_pool("pool.ntp.org".to_string());
        assert_eq!(source_io.pools().len(), 1);

        // Calling again with the same domain is idempotent (no panic)
        source_io.create_pool("pool.ntp.org".to_string());
        assert_eq!(source_io.pools().len(), 1);
    }

    #[test]
    fn create_multiple_pools() {
        let mut source_io = make_source_io();

        source_io.create_pool("pool1.ntp.org".to_string());
        source_io.create_pool("pool2.ntp.org".to_string());
        assert_eq!(source_io.pools().len(), 2);
    }
}
