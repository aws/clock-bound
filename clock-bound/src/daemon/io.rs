//! Perform IO on clock events
//!
//! This module implements the logic needed to retrieve time sync sample measurements, be it from NTP sources from
//! over the internet, the PHC via Linux's `ioctl` interface or some other source.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tokio_util::task::TaskTracker;
use tracing::{debug, error, info, warn};

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

pub mod vmclock;
use vmclock::{VMClock, VMClockParams};

/// Number of times to attempt to initialize the VMClock before giving up and running in the
/// `Failed` state.
const VMCLOCK_INIT_ATTEMPTS: u8 = 3;

/// Delay between VMClock initialization attempts.
const VMCLOCK_INIT_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

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
    dns_message_tx: mpsc::Sender<daemon::DnsMessage>,
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
            dns_message_tx,
            in_use_ip_addrs: InUseIpAddrs::new(),
        }
    }

    /// Initializes the IO task for sampling the Amazon Time Sync NTP source.
    ///
    /// # Panics
    /// - If not called within the `tokio` runtime.
    pub fn create_amazon_time_sync(&mut self, event_sender: async_ring_buffer::Sender<event::Ntp>) {
        debug!("Creating Amazon Time Sync source.");
        debug!(?self.amazon_time_sync, "Current source entry status.");
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

        info!("Amazon Time Sync source registered.");
    }

    /// Initializes the IO task for sampling a specific NTP Server source.
    ///
    /// # Panics
    /// - If not called within the `tokio` runtime.
    /// - If a duplicate IP is added (global to the app)
    pub fn create_ip_addr_source(&mut self, source: ip_addr_source::Sender) {
        let (server_address, event_sender) = source;
        debug!("Creating IP addr source at {}.", server_address.ip());

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

        info!("IP addr source registered at {}.", server_address.ip());
    }

    /// Initializes an NTP pool
    ///
    /// `max_sources` is the maximum number of healthy NTP tasks the pool's
    /// resolver maintains.
    #[tracing::instrument(level = "info", skip(self))]
    pub fn create_pool(&mut self, pool_domain: String, max_sources: usize) {
        debug!("Creating DnsPool IO");

        if self.dns_pools.contains_key(&pool_domain) {
            warn!("DnsPool IO already exists");
            return;
        }

        let dns_pool = Pool::construct(
            pool_domain.clone(),
            self.dns_message_tx.clone(),
            self.in_use_ip_addrs.clone(),
            max_sources,
        );
        self.dns_pools.insert(pool_domain, dns_pool);

        debug!("DnsPool IO constructed");
    }

    /// Pools getter
    pub fn pools(&self) -> &HashMap<String, Pool> {
        &self.dns_pools
    }

    /// Number of tasks currently tracked by the internal task tracker.
    ///
    /// Used by tests to assert that spawning-side effects (e.g. `add_pool_source`
    /// spawning an `NtpSource` task) increment the tracked task count.
    #[cfg(test)]
    pub(crate) fn task_count(&self) -> usize {
        self.task_tracker.len()
    }

    /// Adds and spawns an NTP source within a DNS pool.
    ///
    /// Constructs the `dns::NtpSource`, inserts it into the pool's `HashMap` as Running,
    /// and spawns the task on the internal task tracker.
    ///
    /// # Panics
    /// Panics if
    /// - the pool does not exist.
    /// - the source already exists in the pool
    #[tracing::instrument(level = "info", skip(self, event_sender))]
    pub fn add_pool_source(
        &mut self,
        pool_domain: &str,
        address: SocketAddr,
        event_sender: async_ring_buffer::Sender<event::Ntp>,
    ) {
        let pool = self
            .dns_pools
            .get_mut(pool_domain)
            .expect("Pool does not exist for add_pool_source");

        let (ctrl_sender, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let clock_disruption_receiver = self.clock_disruption_channels.sender.subscribe();
        let resolver_tx = pool.resolver_message_sender();

        let ntp_source = dns::ntp_source::NtpSource::construct(
            pool_domain.to_owned(),
            address,
            event_sender,
            ctrl_receiver,
            clock_disruption_receiver,
            self.selected_clock.clone(),
            self.daemon_info.clone(),
            resolver_tx,
        );

        let source = Source {
            state: SourceState::Running,
            ctrl_sender,
        };
        pool.add_source(address, source);

        // Spawn the NtpSource task — move the constructed runner into the task
        let mut runner = ntp_source;
        self.task_tracker.spawn(async move { runner.run().await });
        info!("DNS pool NTP source registered.");
    }

    /// Removes an NTP source from a DNS pool and sends it a shutdown signal.
    ///
    /// # Panics
    /// Panics if
    /// - the pool does not exist
    /// - the source does not exist in this pool
    #[tracing::instrument(level = "info", skip(self))]
    pub async fn remove_pool_source(&mut self, pool_domain: &str, address: &SocketAddr) {
        let pool = self
            .dns_pools
            .get_mut(pool_domain)
            .expect("Pool does not exist for remove_pool_source");

        let source = pool.remove_source(address);
        source
            .ctrl_sender
            .send(ControlRequest::Shutdown)
            .await
            .unwrap();
        debug!("Sent shutdown to DNS pool NTP source");
    }

    /// Initializes the IO task for sampling the PHC source.
    ///
    /// # Panics
    /// - If not called within the `tokio` runtime.
    pub async fn create_phc(&mut self, event_sender: async_ring_buffer::Sender<event::Phc>) {
        debug!("Creating PHC source.");
        debug!(?self.phc, "Current PHC source entry status.");
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
                        warn!(error = %e, "Failed to construct PHC source.");
                        None
                    }
                }
            };
        }

        if let Some(phc) = self.phc() {
            info!("PHC source registered at {}.", phc.device_path());
        }
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
    /// The VMClock is constructed (no IO) and then initialized, retrying up to
    /// [`VMCLOCK_INIT_ATTEMPTS`] times with [`VMCLOCK_INIT_RETRY_DELAY`] between attempts. The
    /// source is always stored and spawned regardless of whether initialization succeeds: if
    /// initialization fails, the VMClock task runs in the `Failed` state (emitting a periodic
    /// error log), and the shared state reflects `Failed` so the `ClockState` writes
    /// `ClockStatus::Unknown`.
    ///
    /// # Panics
    /// - If not called within the `tokio` runtime.
    pub async fn create_vmclock(&mut self, vmclock_shm_path: &str) {
        debug!("Creating VMClock source.");
        debug!(?self.vmclock, "Current source entry status.");
        if self.vmclock.is_none() {
            self.vmclock = {
                let (ctrl_sender, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);

                debug!("Enabling VMClock.");
                let mut vmclock = VMClock::construct(
                    vmclock_shm_path,
                    ctrl_receiver,
                    self.clock_disruption_channels.sender.clone(),
                );

                for attempt in 1..=VMCLOCK_INIT_ATTEMPTS {
                    match vmclock.initialize().await {
                        Ok(()) => {
                            info!("VMClock initialized successfully.");
                            break;
                        }
                        Err(e) => {
                            warn!(?e, attempt, "Failed to initialize VMClock.");
                            if attempt < VMCLOCK_INIT_ATTEMPTS {
                                tokio::time::sleep(VMCLOCK_INIT_RETRY_DELAY).await;
                            } else {
                                error!(
                                    "VMClock could not be initialized after {VMCLOCK_INIT_ATTEMPTS} attempts. \
                                     Running in Failed state."
                                );
                            }
                        }
                    }
                }

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

    /// Returns the [`VMClockParams`] for the VMClock source, if one exists.
    ///
    /// Returns `Some` whenever a VMClock source has been created (even if it failed to
    /// initialize), so the `ClockState` knows clock disruption support was expected. The shared
    /// state within reflects whether the VMClock is `Running` or `Failed`.
    pub fn vmclock_params(&self) -> Option<VMClockParams> {
        self.vmclock().map(|vmclock| VMClockParams {
            shared_state: vmclock.shared_state(),
            disruption_marker: vmclock.last_disruption_marker(),
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
                    "Attempted to spawn an Amazon Time Sync source when it is currently running."
                );
            }
        } else {
            debug!("Could not spawn an Amazon Time Sync source. No source data provided.");
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
                warn!("Attempted to spawn a PHC source when it is currently running.");
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
                debug!("Spawned VMClock.");
            } else {
                warn!("Attempted to spawn a VMClock source when it is currently running.");
            }
        } else {
            debug!("Could not spawn a VMClock source. No source data provided.");
        }

        // Spawn ip addr sources
        for (key, ip_addr_source) in &mut self.ip_addr_sources {
            debug!("Attempting to spawn {key:?} IP addr source.");
            if let SourceState::Initialized(mut ip_addr_source) =
                ip_addr_source.state.transition_to_running()
            {
                self.task_tracker
                    .spawn(async move { ip_addr_source.run().await });
                debug!("Successfully spawned IP addr source.");
            } else {
                warn!("Attempted to spawn an IP addr source when it is currently running.");
            }
        }

        // Spawn dns pool resolvers
        for (key, pool) in &mut self.dns_pools {
            debug!("Attempting to spawn {key} dns pool resolver.");
            pool.spawn_resolver(&self.task_tracker);
        }

        self.task_tracker.close();
    }

    /// Sends shutdown signals to all io tasks.
    ///
    /// This function sequentially sends `ControlRequest::Shutdown` signals to all
    /// io tasks and then waits for all io tasks to exit.
    pub async fn shutdown_all(&mut self) {
        debug!("Starting shutdown of SourceIO components.");
        // Shutdown Amazon Time Sync source
        if let Some(Source {
            state: _,
            ctrl_sender,
        }) = &mut self.amazon_time_sync
        {
            match ctrl_sender.send(ControlRequest::Shutdown).await {
                Ok(()) => debug!("Successfully sent shutdown signal to Amazon Time Sync source."),
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
                Ok(()) => debug!("Successfully sent shutdown signal to PHC source."),
                Err(e) => warn!(?e, "Failed to send shutdown signal to PHC source."),
            }
        }

        // Shutdown vmclock source
        if let Some(Source {
            state: _,
            ctrl_sender,
        }) = &mut self.vmclock
        {
            match ctrl_sender.send(ControlRequest::Shutdown).await {
                Ok(()) => debug!("Successfully sent shutdown signal to VMClock source."),
                Err(e) => warn!(?e, "Failed to send shutdown signal to VMClock source."),
            }
        }

        // Shutdown ip addr sources
        for ip_addr_source in self.ip_addr_sources.values_mut() {
            match ip_addr_source
                .ctrl_sender
                .send(ControlRequest::Shutdown)
                .await
            {
                Ok(()) => debug!("Successfully sent shutdown signal to IP addr source."),
                Err(e) => warn!(?e, "Failed to send shutdown signal to IP addr source."),
            }
        }

        // Shutdown DNS pools (NTP sources first, then Resolvers)
        for pool in self.dns_pools.values_mut() {
            pool.shutdown().await;
        }

        // Wait for all io tasks to exit
        debug!("Waiting for {} IO tasks to exit.", self.task_tracker.len());
        self.task_tracker.wait().await;
        debug!("All tasks exited. Shutdown of IO complete.");
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
        source_io.create_pool("pool.ntp.org".to_string(), 5);
        assert_eq!(source_io.pools().len(), 1);

        // Calling again with the same domain is idempotent (no panic)
        source_io.create_pool("pool.ntp.org".to_string(), 5);
        assert_eq!(source_io.pools().len(), 1);
    }

    #[test]
    fn create_multiple_pools() {
        let mut source_io = make_source_io();

        source_io.create_pool("pool1.ntp.org".to_string(), 5);
        source_io.create_pool("pool2.ntp.org".to_string(), 5);
        assert_eq!(source_io.pools().len(), 2);
    }

    #[test]
    fn create_pool_shares_source_io_in_use_ip_addrs() {
        use std::net::{IpAddr, Ipv4Addr};

        let mut source_io = make_source_io();
        source_io.create_pool("pool1.ntp.org".to_string(), 5);
        source_io.create_pool("pool2.ntp.org".to_string(), 5);

        // Claim an address through SourceIO's own tracker.
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        source_io.in_use_ip_addrs.add(addr).unwrap();

        // Every pool's resolver must see the claim, proving they all share SourceIO's
        // single tracker rather than holding isolated copies.
        for domain in ["pool1.ntp.org", "pool2.ntp.org"] {
            let pool = source_io.pools().get(domain).expect("pool exists");
            assert!(
                pool.resolver_in_use_ip_addrs_snapshot().contains(&addr),
                "pool '{domain}' resolver does not share SourceIO's in-use IP tracker"
            );
        }
    }
}
