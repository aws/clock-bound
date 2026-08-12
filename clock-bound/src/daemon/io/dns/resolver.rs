//! The DNS task that acts as the resolver for a pool

mod message;
pub use message::Message;

use tracing::{debug, error, info, trace};

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use std::net::Ipv6Addr;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::daemon::io::ControlRequest;

/// Time before re-selecting a bad host
const QUARANTINE_DURATION: Duration = Duration::from_hours(1);

/// Time before re-resolving the domain name
///
/// Kind of an optimization to prevent calling `lookup_host` too frequently
const RESOLVE_CACHE_TTL: Duration = Duration::from_secs(30);

/// The maximum ntp sources to resolve in a pool
const MAX_SOURCES: usize = 5;

/// Manages the lifecycle of NTP IO tasks for a single DNS pool.
///
/// The resolver periodically resolves the pool's domain name to IP addresses
/// and maintains up to [`MAX_SOURCES`] active NTP tasks. When a task reports
/// an address as unreachable, the resolver quarantines it and allows the
/// periodic tick to backfill with a fresh address from the pool.
///
/// Communication flows:
/// - Receives [`ControlRequest`] commands from [`SourceIO`](super::super::SourceIO)
///   (e.g. shutdown).
/// - Receives [`Message`] notifications from its NTP IO tasks (registration,
///   unreachable).
/// - Sends commands to the daemon (via `daemon_tx`) to spawn or tear down NTP
///   IO tasks.
#[derive(Debug)]
pub struct Resolver {
    /// Domain name of the pool
    pool_domain: String,
    /// State of all IO tasks in this DNS pool
    task_state: HashMap<IpAddr, State>,
    /// Hosts that were marked as unhealthy, and shouldn't be chosen for new io tasks
    /// when resolving from the pool
    ///
    /// Mapping of Host Ip Addresses to the timestamp when it was quarantined.
    /// The timestamp is used to cleanup the quarantined field over time
    quarantined: HashMap<IpAddr, Instant>,
    /// Cache of resolved hosts
    last_resolved: Option<LastResolved>,
    /// mpsc TX of daemon messages
    ///
    /// These are used to send commands to the daemon task to
    /// delete or initialize new NTP IO Tasks as well as install into
    /// the clock sync algorithm
    daemon_tx: mpsc::Sender<()>, // TODO: create message
    /// Receiver from the IO front end
    ctrl_receiver: mpsc::Receiver<ControlRequest>,
    /// Receiver from the DNS Pool's NTP tasks
    ntp_receiver: mpsc::Receiver<Message>,
}

impl Resolver {
    #[tracing::instrument(level = "info", skip_all, fields(pool = %self.pool_domain))]
    async fn run(&mut self) {
        info!("Starting run loop");
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                ctrl_req = self.ctrl_receiver.recv() => {
                    match ctrl_req {
                        None => {
                            // SourceIO dropped the ctrl_sender — expected during shutdown.
                            info!("Control channel closed, exiting run loop");
                            break;
                        },
                        Some(ControlRequest::Shutdown) => {
                            info!("Received shutdown signal, exiting run loop");
                            break;
                        },
                    }
                },
                ntp_task_message = self.ntp_receiver.recv() => {
                    let Some(msg) = ntp_task_message else {
                        // All NTP task senders have been dropped — Possible during shutdown.
                        info!("All NTP task senders dropped, exiting run loop");
                        break;
                    };
                    self.handle_ntp_task_message(msg).await;
                }
                _ = interval.tick() => {
                    self.handle_interval_tick().await;
                }
            }
        }
    }

    async fn handle_interval_tick(&mut self) {
        self.cleanup_quarantine();
        self.replenish_pool().await;
    }

    async fn handle_ntp_task_message(&mut self, message: Message) {
        match message {
            Message::RegisterAddr(msg) => self.handle_registration(msg.addr),
            Message::UnreachableAddr(msg) => self.handle_unreachable(msg.addr).await,
            Message::NtpTaskShutdown(msg) => self.handle_ntp_task_shutdown(msg.addr),
        }
    }

    /// Resolve DNS and launch NTP tasks until the pool reaches [`MAX_SOURCES`] healthy members.
    ///
    /// This is periodically called by the run loop. DNS resolution is the sole IO
    /// operation — all other logic is pure state filtering.
    ///
    /// This is the function to reconcile entropy.
    /// This only tries to increase the NTP task count to the expected healthy count.
    /// If unable to resolve enough to meet this need, it will just try again on the next tick.
    ///
    /// Decreasing the number of pools from the `MAX_SOURCES` size is handled by the
    /// NTP IO tasks when they report an unreachable event.
    async fn replenish_pool(&mut self) {
        // bug condition, should never happen or app is malformed
        assert!(MAX_SOURCES >= self.num_expected_healthy());
        let needed_host_count = MAX_SOURCES - self.num_expected_healthy();
        trace!(
            needed = needed_host_count,
            healthy = self.num_expected_healthy(),
            target = MAX_SOURCES,
            "pool state"
        );
        if needed_host_count == 0 {
            return;
        }

        let addrs = match self.cached_resolve_host().await {
            Ok(addrs) => addrs,
            Err(error) => {
                info!(
                    ?error,
                    healthy = self.num_expected_healthy(),
                    target = MAX_SOURCES,
                    "DNS resolution failed, will retry next tick"
                );
                return;
            }
        };

        let ipv6_addr = addrs
            .iter()
            .find_map(|a| match a {
                IpAddr::V4(_) => None,
                IpAddr::V6(a) => Some(a),
            })
            .copied();

        let supports_ipv6 = if let Some(ipv6_addr) = ipv6_addr {
            self.verify_ipv6(ipv6_addr).await
        } else {
            false
        };

        let new_addrs = self.new_addrs_to_launch(addrs, needed_host_count, supports_ipv6);
        debug!(?new_addrs, "Addresses to launch");

        for _addr in new_addrs {
            todo!("send message to daemon for io handling and store in task_state as initializing")
        }
    }

    /// Cleanup task to delete quarantine
    fn cleanup_quarantine(&mut self) {
        let now = Instant::now();
        self.quarantined.retain(|addr, quarantine_time| {
            let should_retain = now.duration_since(*quarantine_time) < QUARANTINE_DURATION;
            if !should_retain {
                info!(%addr, "Removing from quarantine");
            }
            should_retain
        });
    }

    /// Handle a registration request from an NTP io task
    ///
    /// A launched IO task will notify the resolver that it has started up
    #[tracing::instrument(level = "info", skip(self))]
    fn handle_registration(&mut self, addr: IpAddr) {
        let state = self.task_state.get_mut(&addr).unwrap();
        assert_eq!(*state, State::Initializing);
        *state = State::Running;
        info!(%addr, "NTP task registered");
    }

    /// Handle a host becoming unreachable
    ///
    /// Do the work to initiate the deletion of a task. Creation of the new task
    /// will be done by [`Resolver::replenish_pool`].
    #[tracing::instrument(level = "info", skip(self))]
    async fn handle_unreachable(&mut self, addr: IpAddr) {
        // When a host becomes unreachable, we quarantine it for a period of time
        // before allowing it to be selected again. This prevents rapid cycling
        // between healthy and unhealthy states.
        self.quarantined.insert(addr, Instant::now());
        let state = self.task_state.get_mut(&addr).unwrap();
        assert_eq!(*state, State::Running);
        *state = State::Unhealthy;
        info!(
            quarantine_secs = QUARANTINE_DURATION.as_secs(),
            "Quarantining unreachable NTP source"
        );

        todo!("send message to daemon for io handling")
    }

    /// Handle a notification of an NTP task shutdown
    #[tracing::instrument(level = "info", skip(self))]
    fn handle_ntp_task_shutdown(&mut self, addr: IpAddr) {
        let state = self.task_state.remove(&addr);
        // Verify we are actually tracking this.
        // I would LIKE to verify that it is unhealthy. However the shutdown could be from a
        // quarantine or from a ctrl message. Either way, we want to remove it.
        if state.is_none() {
            error!("Address not found");
            panic!("{addr} unable to removed from {}", self.pool_domain);
        }
        info!("Removed address");
    }

    /// Return the number of hosts in Initializing or Running States
    fn num_expected_healthy(&self) -> usize {
        self.task_state
            .iter()
            .filter(|(_, v)| !matches!(v, State::Unhealthy))
            .count()
    }

    /// Determine the new addresses to initialize
    ///
    /// Given a slice of addresses:
    /// - filter out the ones that are already being used
    /// - filter out the ones that are quarantined
    fn new_addrs_to_launch(
        &self,
        candidate_addrs: Vec<IpAddr>,
        needed_host_count: usize,
        supports_ipv6: bool,
    ) -> Vec<IpAddr> {
        let mut available_addrs: Vec<_> = candidate_addrs
            .into_iter()
            // filter out ipv6 if we don't have it enabled
            .filter(|addr| match addr {
                IpAddr::V4(_) => true,
                IpAddr::V6(_) => supports_ipv6,
            })
            // filter out addrs that are already being used or quarantined
            .filter(|addr| {
                !self.task_state.contains_key(addr) && !self.quarantined.contains_key(addr)
            })
            .collect();

        available_addrs.truncate(needed_host_count);

        available_addrs
    }

    /// Return cached IP addrs if still valid, otherwise resolve addresses to cache and return them
    ///
    /// Cache is still valid if within `LAST_RESOLVED_VALID`
    async fn cached_resolve_host(&mut self) -> std::io::Result<Vec<IpAddr>> {
        if let Some(last_resolved) = self.last_resolved.as_ref()
            && last_resolved.is_valid()
        {
            Ok(last_resolved.addrs().to_vec())
        } else {
            let resolved = self.resolve_host().await?;
            let last_resolved = self.last_resolved.insert(resolved);
            debug!(resolved = ?last_resolved, "resolved DNS");
            Ok(last_resolved.addrs().to_owned())
        }
    }

    async fn resolve_host(&self) -> std::io::Result<LastResolved> {
        let iter = tokio::net::lookup_host((self.pool_domain.as_str(), 123)).await?;
        let addrs = iter.map(|socket_addr| socket_addr.ip()).collect();
        Ok(LastResolved {
            addrs,
            as_of: Instant::now(),
        })
    }

    /// Do a sanity check that IPv6 is available on this platform
    ///
    /// Checking via a bind to IPv6 localhost support is compiled in. Calling connect will
    /// verify that if there are any local issues with trying to route to this ipv6 address
    ///
    /// This is a low-dependency and platform independent way of doing this check right now.
    async fn verify_ipv6(&self, server_addr: Ipv6Addr) -> bool {
        let socket = match UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).await {
            Ok(s) => s,
            Err(error) => {
                debug!(?error, %server_addr, "Unable to bind IPv6 socket");
                return false;
            }
        };

        match socket.connect((server_addr, 123)).await {
            Ok(()) => {
                trace!(%server_addr, "IPv6 connectivity verified");
                true
            }
            Err(error) => {
                debug!(?error, %server_addr, "IPv6 connect failed");
                false
            }
        }
    }
}

/// State of each task
///
/// The state of a task naturally moves through Initializing, Running, and Unhealthy
// Rationale on having an Unhealthy state rather than just removing it:
// This allows the Resolver to hold the state of all NTP IO tasks. Whenever we need to
// add commands to report the state of IO, this simplifies the design to just talk to
// the resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Sent a request to the daemon to construct and spawn the IO task
    Initializing,
    /// The task is Running and making NTP requests to the server
    Running,
    /// The task has deemed the NTP source unreachable, and is in the process of closing down
    Unhealthy,
}

/// Cache of the last resolution result
#[derive(Debug, Clone, PartialEq, Eq)]
struct LastResolved {
    /// resolved addresses in a domain
    addrs: Vec<IpAddr>,
    /// Time that this was resolved
    as_of: Instant,
}

impl LastResolved {
    fn is_valid(&self) -> bool {
        self.as_of.elapsed() < RESOLVE_CACHE_TTL
    }

    fn addrs(&self) -> &[IpAddr] {
        &self.addrs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use tokio::sync::mpsc;

    fn make_resolver(
        task_state: HashMap<IpAddr, State>,
        quarantined: HashMap<IpAddr, Instant>,
        last_resolved: Option<LastResolved>,
    ) -> Resolver {
        let (daemon_tx, _rx) = mpsc::channel(1);
        let (_ctrl_tx, ctrl_receiver) = mpsc::channel(1);
        let (_ntp_tx, ntp_receiver) = mpsc::channel(1);
        Resolver {
            pool_domain: "pool.ntp.org".to_string(),
            task_state,
            quarantined,
            last_resolved,
            daemon_tx,
            ctrl_receiver,
            ntp_receiver,
        }
    }

    #[test]
    fn last_resolved_is_valid_when_recent() {
        let resolved = LastResolved {
            addrs: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
            as_of: Instant::now(),
        };
        assert!(resolved.is_valid());
    }

    #[test]
    fn last_resolved_is_invalid_when_stale() {
        let resolved = LastResolved {
            addrs: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
            as_of: Instant::now() - RESOLVE_CACHE_TTL - Duration::from_secs(1),
        };
        assert!(!resolved.is_valid());
    }

    #[test]
    fn num_expected_healthy_counts_initializing_and_running() {
        let addr1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let addr2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        let addr3 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3));

        let task_state = HashMap::from([
            (addr1, State::Initializing),
            (addr2, State::Running),
            (addr3, State::Unhealthy),
        ]);

        let resolver = make_resolver(task_state, HashMap::new(), None);
        assert_eq!(resolver.num_expected_healthy(), 2);
    }

    #[test]
    fn num_expected_healthy_zero_when_all_unhealthy() {
        let addr1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let addr2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        let task_state = HashMap::from([(addr1, State::Unhealthy), (addr2, State::Unhealthy)]);

        let resolver = make_resolver(task_state, HashMap::new(), None);
        assert_eq!(resolver.num_expected_healthy(), 0);
    }

    #[test]
    fn num_expected_healthy_zero_when_empty() {
        let resolver = make_resolver(HashMap::new(), HashMap::new(), None);
        assert_eq!(resolver.num_expected_healthy(), 0);
    }

    #[test]
    fn new_addrs_to_launch_filters_existing_task_state_addrs() {
        let existing = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let fresh = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        let task_state = HashMap::from([(existing, State::Running)]);
        let resolver = make_resolver(task_state, HashMap::new(), None);

        let result = resolver.new_addrs_to_launch(vec![existing, fresh], 5, true);
        assert_eq!(result, vec![fresh]);
    }

    #[test]
    fn new_addrs_to_launch_filters_quarantined_addrs() {
        let quarantined_addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let fresh = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        let quarantined = HashMap::from([(quarantined_addr, Instant::now())]);
        let resolver = make_resolver(HashMap::new(), quarantined, None);

        let result = resolver.new_addrs_to_launch(vec![quarantined_addr, fresh], 5, true);
        assert_eq!(result, vec![fresh]);
    }

    #[test]
    fn new_addrs_to_launch_filters_ipv6_when_not_supported() {
        let v4 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let v6 = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));

        let resolver = make_resolver(HashMap::new(), HashMap::new(), None);

        let result = resolver.new_addrs_to_launch(vec![v4, v6], 5, false);
        assert_eq!(result, vec![v4]);
    }

    #[test]
    fn new_addrs_to_launch_keeps_ipv6_when_supported() {
        let v4 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let v6 = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));

        let resolver = make_resolver(HashMap::new(), HashMap::new(), None);

        let result = resolver.new_addrs_to_launch(vec![v4, v6], 5, true);
        assert_eq!(result, vec![v4, v6]);
    }

    #[test]
    fn new_addrs_to_launch_truncates_to_needed_count() {
        let addrs: Vec<IpAddr> = (1..=10)
            .map(|i| IpAddr::V4(Ipv4Addr::new(10, 0, 0, i)))
            .collect();

        let resolver = make_resolver(HashMap::new(), HashMap::new(), None);

        let result = resolver.new_addrs_to_launch(addrs, 3, true);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn new_addrs_to_launch_returns_empty_when_all_filtered() {
        let existing = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let quarantined_addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        let task_state = HashMap::from([(existing, State::Initializing)]);
        let quarantined = HashMap::from([(quarantined_addr, Instant::now())]);
        let resolver = make_resolver(task_state, quarantined, None);

        let result = resolver.new_addrs_to_launch(vec![existing, quarantined_addr], 5, true);
        assert!(result.is_empty());
    }

    #[test]
    fn new_addrs_to_launch_returns_empty_when_needed_count_is_zero() {
        let v4 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let resolver = make_resolver(HashMap::new(), HashMap::new(), None);

        let result = resolver.new_addrs_to_launch(vec![v4], 0, true);
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn cached_resolve_host_returns_cached_addrs_when_valid() {
        let addrs = vec![
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
        ];

        let last_resolved = LastResolved {
            addrs: addrs.clone(),
            as_of: Instant::now(),
        };

        let mut resolver = make_resolver(HashMap::new(), HashMap::new(), Some(last_resolved));

        // Should return the cached value without calling resolve_host
        let result = resolver.cached_resolve_host().await.unwrap();
        assert_eq!(result, addrs);
    }

    #[test]
    fn cleanup_quarantine_removes_expired_entries() {
        let expired_addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let fresh_addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        let quarantined = HashMap::from([
            // quarantined well beyond QUARANTINE_DURATION ago
            (
                expired_addr,
                Instant::now() - QUARANTINE_DURATION - Duration::from_secs(1),
            ),
            // quarantined just now
            (fresh_addr, Instant::now()),
        ]);

        let mut resolver = make_resolver(HashMap::new(), quarantined, None);
        resolver.cleanup_quarantine();

        assert!(!resolver.quarantined.contains_key(&expired_addr));
        assert!(resolver.quarantined.contains_key(&fresh_addr));
    }

    #[test]
    fn cleanup_quarantine_retains_all_when_none_expired() {
        let addr1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let addr2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        let quarantined = HashMap::from([
            (addr1, Instant::now()),
            (addr2, Instant::now() - Duration::from_secs(60)),
        ]);

        let mut resolver = make_resolver(HashMap::new(), quarantined, None);
        resolver.cleanup_quarantine();

        assert_eq!(resolver.quarantined.len(), 2);
    }

    #[test]
    fn cleanup_quarantine_removes_all_when_all_expired() {
        let addr1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let addr2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        let quarantined = HashMap::from([
            (
                addr1,
                Instant::now() - QUARANTINE_DURATION - Duration::from_secs(100),
            ),
            (
                addr2,
                Instant::now() - QUARANTINE_DURATION - Duration::from_secs(1),
            ),
        ]);

        let mut resolver = make_resolver(HashMap::new(), quarantined, None);
        resolver.cleanup_quarantine();

        assert!(resolver.quarantined.is_empty());
    }

    #[test]
    fn cleanup_quarantine_is_noop_when_empty() {
        let mut resolver = make_resolver(HashMap::new(), HashMap::new(), None);
        resolver.cleanup_quarantine();

        assert!(resolver.quarantined.is_empty());
    }

    #[test]
    fn handle_registration_transitions_initializing_to_running() {
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let task_state = HashMap::from([(addr, State::Initializing)]);

        let mut resolver = make_resolver(task_state, HashMap::new(), None);
        resolver.handle_registration(addr);

        assert_eq!(*resolver.task_state.get(&addr).unwrap(), State::Running);
    }

    #[test]
    #[should_panic]
    fn handle_registration_panics_when_already_running() {
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let task_state = HashMap::from([(addr, State::Running)]);

        let mut resolver = make_resolver(task_state, HashMap::new(), None);
        resolver.handle_registration(addr);
    }

    #[test]
    #[should_panic]
    fn handle_registration_panics_when_unhealthy() {
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let task_state = HashMap::from([(addr, State::Unhealthy)]);

        let mut resolver = make_resolver(task_state, HashMap::new(), None);
        resolver.handle_registration(addr);
    }

    #[test]
    #[should_panic]
    fn handle_registration_panics_when_addr_not_in_task_state() {
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        let mut resolver = make_resolver(HashMap::new(), HashMap::new(), None);
        resolver.handle_registration(addr);
    }

    #[test]
    fn handle_registration_does_not_affect_other_tasks() {
        let addr1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let addr2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        let addr3 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3));

        let task_state = HashMap::from([
            (addr1, State::Initializing),
            (addr2, State::Running),
            (addr3, State::Unhealthy),
        ]);

        let mut resolver = make_resolver(task_state, HashMap::new(), None);
        resolver.handle_registration(addr1);

        assert_eq!(*resolver.task_state.get(&addr1).unwrap(), State::Running);
        assert_eq!(*resolver.task_state.get(&addr2).unwrap(), State::Running);
        assert_eq!(*resolver.task_state.get(&addr3).unwrap(), State::Unhealthy);
    }
}
