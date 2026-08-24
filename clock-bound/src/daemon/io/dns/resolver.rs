//! The DNS task that acts as the resolver for a pool

mod message;
pub use message::Message;

use tracing::{debug, error, info, trace, warn};

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use std::net::Ipv6Addr;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::daemon::RunLoopControl;
use crate::daemon::io::ControlRequest;
use crate::daemon::io::InUseIpAddrs;
use crate::daemon::message::{AddPoolAddr, Dns as DnsMessage, RemovePoolAddr};

/// Time before re-selecting a bad host
const QUARANTINE_DURATION: Duration = Duration::from_hours(1);

/// Time before re-resolving the domain name
///
/// Kind of an optimization to prevent calling `lookup_host` too frequently
const RESOLVE_CACHE_TTL: Duration = Duration::from_secs(30);

/// Timeout for `tokio::net::lookup_host` calls.
///
/// If DNS resolution takes longer than this, the call is abandoned and retried
/// on the next tick.
const LOOKUP_HOST_TIMEOUT: Duration = Duration::from_secs(1);

/// Manages the lifecycle of NTP IO tasks for a single DNS pool.
///
/// The resolver periodically resolves the pool's domain name to IP addresses
/// and maintains up to `max_sources` active NTP tasks. When a task reports
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
    daemon_tx: mpsc::Sender<DnsMessage>,
    /// Receiver from the IO front end
    ctrl_receiver: mpsc::Receiver<ControlRequest>,
    /// Receiver from the DNS Pool's NTP tasks
    ntp_receiver: mpsc::Receiver<Message>,
    /// Shared tracker of globally in-use IP addresses across all resolvers
    in_use_ip_addrs: InUseIpAddrs,
    /// Maximum number of healthy NTP tasks this resolver maintains for the pool.
    ///
    /// The periodic tick backfills up to this many sources. A multi-source pool
    /// uses a value greater than one; a single-host domain uses one.
    max_sources: usize,
}

impl Resolver {
    /// Constructs a new `Resolver` for the given pool domain.
    ///
    /// `max_sources` is the maximum number of healthy NTP tasks the resolver
    /// maintains for the pool.
    pub fn construct(
        pool_domain: String,
        daemon_tx: mpsc::Sender<DnsMessage>,
        ctrl_receiver: mpsc::Receiver<ControlRequest>,
        ntp_receiver: mpsc::Receiver<Message>,
        in_use_ip_addrs: InUseIpAddrs,
        max_sources: usize,
    ) -> Self {
        Self {
            pool_domain,
            task_state: HashMap::new(),
            quarantined: HashMap::new(),
            last_resolved: None,
            daemon_tx,
            ctrl_receiver,
            ntp_receiver,
            in_use_ip_addrs,
            max_sources,
        }
    }

    #[tracing::instrument(level = "info", skip_all, fields(pool = %self.pool_domain))]
    pub async fn run(&mut self) {
        debug!("Starting DNS resolver run loop.");
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                ctrl_req = self.ctrl_receiver.recv() => {
                    match ctrl_req {
                        None => {
                            // SourceIO dropped the ctrl_sender — expected during shutdown.
                            debug!("Control channel closed. Exiting run loop.");
                            break;
                        },
                        Some(ControlRequest::Shutdown) => {
                            debug!("Received shutdown signal. Exiting.");
                            break;
                        },
                    }
                },
                ntp_task_message = self.ntp_receiver.recv() => {
                    let Some(msg) = ntp_task_message else {
                        // All NTP task senders have been dropped — Possible during shutdown.
                        debug!("All NTP task senders dropped. Exiting run loop.");
                        break;
                    };
                    if self.handle_ntp_task_message(msg).await == RunLoopControl::Exit {
                        break;
                    }
                }
                _ = interval.tick() => {
                    if self.handle_interval_tick().await == RunLoopControl::Exit {
                        break;
                    }
                }
            }
        }
    }

    async fn handle_interval_tick(&mut self) -> RunLoopControl {
        self.cleanup_quarantine();
        self.replenish_pool().await
    }

    /// Sends a message to the daemon, aborting if a shutdown signal arrives.
    ///
    /// The `daemon_tx` channel has bounded capacity. During shutdown the daemon
    /// stops draining `dns_message_rx`, which can cause the channel to fill.
    /// If the Resolver suspends inside a plain `daemon_tx.send().await`, it will
    /// never return to the `select!` loop to observe the shutdown signal on
    /// `ctrl_receiver` — creating a deadlock where `shutdown_all` waits on the
    /// task tracker while the Resolver is parked on a full channel that will
    /// never drain.
    ///
    /// This helper wraps the send in a `tokio::select!` that races the send
    /// against the control channel, ensuring the Resolver can always exit
    /// promptly when shutdown is requested.
    ///
    /// Returns [`RunLoopControl::Continue`] if the message was sent successfully,
    /// or [`RunLoopControl::Exit`] if a shutdown signal was received (the caller
    /// should exit the run loop).
    async fn send_daemon_message(&mut self, message: DnsMessage) -> RunLoopControl {
        tokio::select! {
            result = self.daemon_tx.send(message) => {
                result.expect("Daemon receiver dropped while Resolver is running");
                RunLoopControl::Continue
            }
            ctrl_req = self.ctrl_receiver.recv() => {
                match ctrl_req {
                    Some(ControlRequest::Shutdown) => {
                        info!("Received shutdown while sending daemon message, exiting");
                    }
                    None => {
                        info!("Control channel closed while sending daemon message, exiting");
                    }
                }
                RunLoopControl::Exit
            }
        }
    }

    async fn handle_ntp_task_message(&mut self, message: Message) -> RunLoopControl {
        match message {
            Message::RegisterAddr(msg) => {
                self.handle_registration(msg.addr);
                RunLoopControl::Continue
            }
            Message::UnreachableAddr(msg) => self.handle_unreachable(msg.addr).await,
            Message::NtpTaskShutdown(msg) => {
                self.handle_ntp_task_shutdown(msg.addr);
                RunLoopControl::Continue
            }
        }
    }

    /// Resolve DNS and launch NTP tasks until the pool reaches `max_sources` healthy members.
    ///
    /// This is periodically called by the run loop. DNS resolution is the sole IO
    /// operation — all other logic is pure state filtering.
    ///
    /// This is the function to reconcile entropy.
    /// This only tries to increase the NTP task count to the expected healthy count.
    /// If unable to resolve enough to meet this need, it will just try again on the next tick.
    ///
    /// Decreasing the number of pools from the `max_sources` size is handled by the
    /// NTP IO tasks when they report an unreachable event.
    ///
    /// Returns [`RunLoopControl::Continue`] if the function completed normally, or
    /// [`RunLoopControl::Exit`] if a shutdown signal was received during a send to
    /// the daemon.
    async fn replenish_pool(&mut self) -> RunLoopControl {
        // bug condition, should never happen or app is malformed
        assert!(self.max_sources >= self.num_expected_healthy());
        let needed_host_count = self.max_sources - self.num_expected_healthy();
        trace!(
            needed = needed_host_count,
            healthy = self.num_expected_healthy(),
            target = self.max_sources,
            "Pool state."
        );
        if needed_host_count == 0 {
            return RunLoopControl::Continue;
        }

        let addrs = match self.cached_resolve_host().await {
            Ok(addrs) => addrs,
            Err(error) => {
                info!(
                    ?error,
                    healthy = self.num_expected_healthy(),
                    target = self.max_sources,
                    "DNS resolution failed. Will retry next tick."
                );
                return RunLoopControl::Continue;
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
        debug!(?new_addrs, "Addresses to launch.");

        for addr in new_addrs {
            let socket_addr = SocketAddr::new(addr, 123);
            let message = DnsMessage::AddPoolAddr(AddPoolAddr {
                pool_domain: self.pool_domain.clone(),
                addr: socket_addr,
            });
            if self.send_daemon_message(message).await == RunLoopControl::Exit {
                return RunLoopControl::Exit;
            }
            self.task_state.insert(addr, State::Initializing);
            debug!(%addr, "Sent AddPoolAddr to daemon and marked as Initializing");
        }
        RunLoopControl::Continue
    }

    /// Cleanup task to delete quarantine
    fn cleanup_quarantine(&mut self) {
        let now = Instant::now();
        self.quarantined.retain(|addr, quarantine_time| {
            let should_retain = now.duration_since(*quarantine_time) < QUARANTINE_DURATION;
            if !should_retain {
                info!(%addr, "Removing from quarantine.");
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
        debug!(%addr, "NTP task registered.");
    }

    /// Handle a host becoming unreachable
    ///
    /// Marks the address as [`State::Unhealthy`].
    ///
    /// Releasing the global in-use claim, removing the `task_state` entry, and
    /// quarantining the address are all deferred to
    /// [`Resolver::handle_ntp_task_shutdown`], which runs once the task has actually
    /// exited. The task is still alive at this point, so its resources must not be
    /// released yet.
    ///
    /// Returns [`RunLoopControl::Continue`] if the function completed normally, or
    /// [`RunLoopControl::Exit`] if a shutdown signal was received during a send to
    /// the daemon.
    #[tracing::instrument(level = "info", skip(self))]
    async fn handle_unreachable(&mut self, addr: IpAddr) -> RunLoopControl {
        // Mark the source unhealthy so it no longer counts as an expected-healthy
        // member of the pool. The `task_state` entry itself is left in place (and
        // still blocks re-selection) until the task's shutdown notification arrives.
        let state = self.task_state.get_mut(&addr).unwrap();
        assert_eq!(*state, State::Running);
        *state = State::Unhealthy;

        let socket_addr = SocketAddr::new(addr, 123);
        let message = DnsMessage::RemovePoolAddr(RemovePoolAddr {
            pool_domain: self.pool_domain.clone(),
            addr: socket_addr,
        });
        if self.send_daemon_message(message).await == RunLoopControl::Exit {
            return RunLoopControl::Exit;
        }

        info!(%addr, "Marked source unhealthy and sent RemovePoolAddr to daemon.");
        RunLoopControl::Continue
    }

    /// Handle a notification of an NTP task shutdown
    ///
    /// This is the single authoritative teardown point for a source. It runs once
    /// the NTP task has actually exited, so it is safe to release everything the
    /// task held.
    #[tracing::instrument(level = "info", skip(self))]
    fn handle_ntp_task_shutdown(&mut self, addr: IpAddr) {
        // Verify we are actually tracking this.
        let Some(state) = self.task_state.remove(&addr) else {
            error!(%addr, "Address not found in task_state.");
            panic!("{addr} unable to be removed from {}.", self.pool_domain);
        };

        // Only quarantine sources torn down because they became unreachable.
        // When a host becomes unreachable, we quarantine it for a period of time
        // before allowing it to be selected again. This prevents rapid cycling
        // between healthy and unhealthy states.
        if state == State::Unhealthy {
            self.quarantined.insert(addr, Instant::now());
            info!(
                quarantine_secs = QUARANTINE_DURATION.as_secs(),
                %addr,
                "Quarantining unhealthy NTP source."
            );
        }

        // Release the address from the global in-use tracker so other resolvers
        // (or this one on a future tick) can potentially select it after quarantine expires.
        if self.in_use_ip_addrs.remove(&addr).is_err() {
            // global radius could potentially be removed elsewhere in the app
            warn!(%addr, "Address was not found in global in-use tracker.");
        }

        info!(%addr, "Removed address.");
    }

    /// Return the number of hosts in Initializing or Running States
    fn num_expected_healthy(&self) -> usize {
        self.task_state
            .iter()
            .filter(|(_, v)| !matches!(v, State::Unhealthy))
            .count()
    }

    /// Determine the new addresses to initialize, and register them in `in_use_ip_addrs` as well as `task_state`
    ///
    /// Given a slice of addresses:
    /// - filter out the ones that are already being used
    /// - filter out the ones that are quarantined
    /// - filter out the ones that are globally in use (tracked by [`InUseIpAddrs`])
    ///
    /// Atomically claims the selected addresses in the shared [`InUseIpAddrs`] tracker
    /// to prevent other resolvers from selecting the same addresses concurrently.
    fn new_addrs_to_launch(
        &mut self,
        candidate_addrs: Vec<IpAddr>,
        needed_host_count: usize,
        supports_ipv6: bool,
    ) -> Vec<IpAddr> {
        // Pre-filter candidates based on local state (ipv6 support, task_state, quarantine)
        let locally_filtered: Vec<_> = candidate_addrs
            .into_iter()
            // filter out ipv6 if we don't have it enabled
            .filter(|addr| match addr {
                IpAddr::V4(_) => true,
                IpAddr::V6(_) => supports_ipv6,
            })
            // filter out addrs that are already being used locally or quarantined
            .filter(|addr| {
                !self.task_state.contains_key(addr) && !self.quarantined.contains_key(addr)
            })
            .collect();

        // Atomically filter out globally in-use addresses and claim the new ones
        let new_addrs = self.in_use_ip_addrs.transact_add(|globally_in_use| {
            let mut available: Vec<_> = locally_filtered
                .iter()
                .filter(|addr| !globally_in_use.contains(addr))
                .copied()
                .collect();

            available.truncate(needed_host_count);
            available
        });
        debug!(
            "Updated global in-use IP addresses: {:?}",
            self.in_use_ip_addrs
        );
        for addr in &new_addrs {
            self.task_state.insert(*addr, State::Initializing);
            debug!(%addr, "Claimed address");
        }
        new_addrs
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
            debug!(resolved = ?last_resolved, "Resolved DNS.");
            Ok(last_resolved.addrs().to_owned())
        }
    }

    async fn resolve_host(&self) -> std::io::Result<LastResolved> {
        let iter = tokio::time::timeout(
            LOOKUP_HOST_TIMEOUT,
            tokio::net::lookup_host((self.pool_domain.as_str(), 123)),
        )
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "DNS lookup timed out"))??;
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
                debug!(?error, %server_addr, "Unable to bind IPv6 socket.");
                return false;
            }
        };

        match socket.connect((server_addr, 123)).await {
            Ok(()) => {
                trace!(%server_addr, "IPv6 connectivity verified.");
                true
            }
            Err(error) => {
                debug!(?error, %server_addr, "IPv6 connect failed.");
                false
            }
        }
    }
}

/// Handles retained by a test that drives a [`Resolver`] built with
/// [`Resolver::new_running`].
///
/// Holding the control sender alive keeps `ctrl_receiver.recv()` pending inside
/// `send_daemon_message`, so the send-vs-shutdown race resolves in favour of the send
/// (returning [`RunLoopControl::Continue`]) rather than observing a closed control
/// channel (which would return [`RunLoopControl::Exit`]).
#[cfg(test)]
pub(crate) struct RunningResolverHandles {
    /// Plays the role of an NTP IO task: send [`Message`]s to the resolver.
    pub(crate) resolver_message_tx: mpsc::Sender<Message>,
    /// Kept alive purely so the resolver's control channel does not appear closed.
    _ctrl_tx: mpsc::Sender<ControlRequest>,
}

#[cfg(test)]
impl Resolver {
    /// Construct a `Resolver` pre-seeded with a single `Running` NTP source.
    ///
    /// This bypasses the normal DNS-driven registration flow (`replenish_pool` ->
    /// `new_addrs_to_launch` -> `handle_registration`) so a test can hand the resolver
    /// an [`Message::UnreachableAddr`] for `addr` without tripping the state assertions
    /// in `handle_unreachable`. The address is seeded as [`State::Running`] in
    /// `task_state` and claimed in the shared [`InUseIpAddrs`] tracker, matching the
    /// state the resolver would hold for a live source.
    ///
    /// Returns the resolver together with the retained sender the caller uses to play
    /// the role of the NTP IO task.
    pub(crate) fn new_running(
        pool_domain: String,
        addr: IpAddr,
        daemon_tx: mpsc::Sender<DnsMessage>,
    ) -> (Self, RunningResolverHandles) {
        let (resolver_message_tx, ntp_receiver) = mpsc::channel::<Message>(16);
        let (ctrl_tx, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);

        let in_use_ip_addrs = InUseIpAddrs::new();
        in_use_ip_addrs
            .add(addr)
            .expect("fresh tracker cannot already contain addr");

        let resolver = Self {
            pool_domain,
            task_state: HashMap::from([(addr, State::Running)]),
            quarantined: HashMap::new(),
            last_resolved: None,
            daemon_tx,
            ctrl_receiver,
            ntp_receiver,
            in_use_ip_addrs,
            max_sources: 5,
        };

        let handles = RunningResolverHandles {
            resolver_message_tx,
            _ctrl_tx: ctrl_tx,
        };

        (resolver, handles)
    }

    /// Receive exactly one message on `ntp_receiver` and handle it synchronously.
    ///
    /// Lets a test drive the resolver deterministically without spawning its run loop:
    /// the awaited `recv` resolves as soon as the caller has sent a message, and the
    /// message is dispatched through the same `handle_ntp_task_message` path the run
    /// loop uses (e.g. emitting `RemovePoolAddr` on `daemon_tx` for an unreachable addr).
    pub(crate) async fn recv_and_handle_one(&mut self) -> RunLoopControl {
        let message = self.ntp_receiver.recv().await.expect("sender dropped");
        self.handle_ntp_task_message(message).await
    }

    /// Test-only snapshot of the shared in-use IP tracker held by this resolver.
    pub(crate) fn in_use_ip_addrs_snapshot(&self) -> Vec<IpAddr> {
        self.in_use_ip_addrs.snapshot()
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
        make_resolver_with_in_use(task_state, quarantined, last_resolved, InUseIpAddrs::new())
    }

    fn make_resolver_with_in_use(
        task_state: HashMap<IpAddr, State>,
        quarantined: HashMap<IpAddr, Instant>,
        last_resolved: Option<LastResolved>,
        in_use_ip_addrs: InUseIpAddrs,
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
            in_use_ip_addrs,
            max_sources: 5,
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
        let mut resolver = make_resolver(task_state, HashMap::new(), None);

        let result = resolver.new_addrs_to_launch(vec![existing, fresh], 5, true);
        assert_eq!(result, vec![fresh]);

        // task_state should contain the original entry plus the newly launched addr
        assert_eq!(*resolver.task_state.get(&existing).unwrap(), State::Running);
        assert_eq!(
            *resolver.task_state.get(&fresh).unwrap(),
            State::Initializing
        );
        // in_use_ip_addrs should track the fresh addr
        let snapshot = resolver.in_use_ip_addrs.snapshot();
        assert!(snapshot.contains(&fresh));
        assert!(!snapshot.contains(&existing));
    }

    #[test]
    fn new_addrs_to_launch_filters_quarantined_addrs() {
        let quarantined_addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let fresh = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        let quarantined = HashMap::from([(quarantined_addr, Instant::now())]);
        let mut resolver = make_resolver(HashMap::new(), quarantined, None);

        let result = resolver.new_addrs_to_launch(vec![quarantined_addr, fresh], 5, true);
        assert_eq!(result, vec![fresh]);

        // task_state should contain only the fresh addr
        assert!(!resolver.task_state.contains_key(&quarantined_addr));
        assert_eq!(
            *resolver.task_state.get(&fresh).unwrap(),
            State::Initializing
        );
        // in_use_ip_addrs should track only the fresh addr
        let snapshot = resolver.in_use_ip_addrs.snapshot();
        assert!(snapshot.contains(&fresh));
        assert!(!snapshot.contains(&quarantined_addr));
    }

    #[test]
    fn new_addrs_to_launch_filters_ipv6_when_not_supported() {
        let v4 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let v6 = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));

        let mut resolver = make_resolver(HashMap::new(), HashMap::new(), None);

        let result = resolver.new_addrs_to_launch(vec![v4, v6], 5, false);
        assert_eq!(result, vec![v4]);

        // task_state should contain only the v4 addr
        assert_eq!(*resolver.task_state.get(&v4).unwrap(), State::Initializing);
        assert!(!resolver.task_state.contains_key(&v6));
        // in_use_ip_addrs should track only v4
        let snapshot = resolver.in_use_ip_addrs.snapshot();
        assert!(snapshot.contains(&v4));
        assert!(!snapshot.contains(&v6));
    }

    #[test]
    fn new_addrs_to_launch_keeps_ipv6_when_supported() {
        let v4 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let v6 = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));

        let mut resolver = make_resolver(HashMap::new(), HashMap::new(), None);

        let result = resolver.new_addrs_to_launch(vec![v4, v6], 5, true);
        assert_eq!(result, vec![v4, v6]);

        // task_state should contain both addresses
        assert_eq!(*resolver.task_state.get(&v4).unwrap(), State::Initializing);
        assert_eq!(*resolver.task_state.get(&v6).unwrap(), State::Initializing);
        // in_use_ip_addrs should track both
        let snapshot = resolver.in_use_ip_addrs.snapshot();
        assert!(snapshot.contains(&v4));
        assert!(snapshot.contains(&v6));
    }

    #[test]
    fn new_addrs_to_launch_truncates_to_needed_count() {
        let addrs: Vec<IpAddr> = (1..=10)
            .map(|i| IpAddr::V4(Ipv4Addr::new(10, 0, 0, i)))
            .collect();

        let mut resolver = make_resolver(HashMap::new(), HashMap::new(), None);

        let result = resolver.new_addrs_to_launch(addrs, 3, true);
        assert_eq!(result.len(), 3);

        // task_state should contain exactly the 3 launched addresses
        assert_eq!(resolver.task_state.len(), 3);
        for addr in &result {
            assert_eq!(*resolver.task_state.get(addr).unwrap(), State::Initializing);
        }
        // in_use_ip_addrs should track the same 3 addresses
        let snapshot = resolver.in_use_ip_addrs.snapshot();
        assert_eq!(snapshot.len(), 3);
        for addr in &result {
            assert!(snapshot.contains(addr));
        }
    }

    #[test]
    fn new_addrs_to_launch_returns_empty_when_all_filtered() {
        let existing = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let quarantined_addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        let task_state = HashMap::from([(existing, State::Initializing)]);
        let quarantined = HashMap::from([(quarantined_addr, Instant::now())]);
        let mut resolver = make_resolver(task_state, quarantined, None);

        let result = resolver.new_addrs_to_launch(vec![existing, quarantined_addr], 5, true);
        assert!(result.is_empty());

        // task_state should remain unchanged (only the pre-existing entry)
        assert_eq!(resolver.task_state.len(), 1);
        assert_eq!(
            *resolver.task_state.get(&existing).unwrap(),
            State::Initializing
        );
        // in_use_ip_addrs should remain empty (nothing was claimed)
        assert!(resolver.in_use_ip_addrs.snapshot().is_empty());
    }

    #[test]
    fn new_addrs_to_launch_returns_empty_when_needed_count_is_zero() {
        let v4 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let mut resolver = make_resolver(HashMap::new(), HashMap::new(), None);

        let result = resolver.new_addrs_to_launch(vec![v4], 0, true);
        assert!(result.is_empty());

        // task_state should remain empty
        assert!(resolver.task_state.is_empty());
        // in_use_ip_addrs should remain empty
        assert!(resolver.in_use_ip_addrs.snapshot().is_empty());
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

    #[test]
    fn new_addrs_to_launch_filters_globally_in_use_addrs() {
        let globally_used = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let fresh = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        let in_use = InUseIpAddrs::new();
        in_use.add(globally_used).unwrap();

        let mut resolver = make_resolver_with_in_use(HashMap::new(), HashMap::new(), None, in_use);

        let result = resolver.new_addrs_to_launch(vec![globally_used, fresh], 5, true);
        assert_eq!(result, vec![fresh]);
    }

    #[test]
    fn new_addrs_to_launch_claims_addrs_in_shared_tracker() {
        let addr1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let addr2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        let in_use = InUseIpAddrs::new();
        let mut resolver =
            make_resolver_with_in_use(HashMap::new(), HashMap::new(), None, in_use.clone());

        let result = resolver.new_addrs_to_launch(vec![addr1, addr2], 5, true);
        assert_eq!(result, vec![addr1, addr2]);

        // Verify the addresses were claimed in the shared tracker
        let snapshot = in_use.snapshot();
        assert!(snapshot.contains(&addr1));
        assert!(snapshot.contains(&addr2));
    }

    #[test]
    fn new_addrs_to_launch_does_not_claim_when_all_filtered() {
        let existing = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        let in_use = InUseIpAddrs::new();
        let task_state = HashMap::from([(existing, State::Running)]);
        let mut resolver =
            make_resolver_with_in_use(task_state, HashMap::new(), None, in_use.clone());

        let result = resolver.new_addrs_to_launch(vec![existing], 5, true);
        assert!(result.is_empty());

        // Nothing should have been added to the shared tracker
        assert!(in_use.snapshot().is_empty());
    }

    #[tokio::test]
    async fn handle_unreachable_marks_unhealthy_and_notifies_daemon() {
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        let in_use = InUseIpAddrs::new();
        in_use.add(addr).unwrap();

        let task_state = HashMap::from([(addr, State::Running)]);
        let mut resolver =
            make_resolver_with_in_use(task_state, HashMap::new(), None, in_use.clone());

        // Keep a live daemon receiver so the RemovePoolAddr send succeeds instead of
        // panicking on a dropped receiver.
        let (daemon_tx, mut daemon_rx) = mpsc::channel(1);
        resolver.daemon_tx = daemon_tx;

        // Keep a live control sender so `send_daemon_message` does not observe a closed
        // control channel and short-circuit to `Exit`.
        let (_ctrl_tx, ctrl_receiver) = mpsc::channel(1);
        resolver.ctrl_receiver = ctrl_receiver;

        let control = resolver.handle_unreachable(addr).await;
        assert_eq!(control, RunLoopControl::Continue);

        // The address is marked Unhealthy but its task_state entry is retained; it is
        // only removed once the task's shutdown notification arrives.
        assert_eq!(resolver.task_state.get(&addr), Some(&State::Unhealthy));

        // The global claim must NOT be released here: the task is still alive during
        // teardown, so releasing it would let another resolver launch a duplicate.
        assert!(in_use.snapshot().contains(&addr));

        // Quarantine is deferred to the shutdown handler, so nothing is quarantined yet.
        assert!(!resolver.quarantined.contains_key(&addr));

        // A RemovePoolAddr message should have been sent to the daemon.
        let message = daemon_rx.recv().await.expect("daemon message sent");
        match message {
            DnsMessage::RemovePoolAddr(remove) => {
                assert_eq!(remove.addr, SocketAddr::new(addr, 123));
                assert_eq!(remove.pool_domain, "pool.ntp.org");
            }
            other => panic!("expected RemovePoolAddr, got {other:?}"),
        }
    }

    #[test]
    fn handle_source_shutdown_when_unhealthy_releases_tracker_and_quarantines() {
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        // An unreachable-driven teardown: the source was marked Unhealthy and still
        // holds its global claim when the shutdown notification arrives.
        let in_use = InUseIpAddrs::new();
        in_use.add(addr).unwrap();
        let task_state = HashMap::from([(addr, State::Unhealthy)]);
        let mut resolver =
            make_resolver_with_in_use(task_state, HashMap::new(), None, in_use.clone());

        resolver.handle_ntp_task_shutdown(addr);

        // The task is gone: remove from task_state and release the global claim.
        assert!(!resolver.task_state.contains_key(&addr));
        assert!(!in_use.snapshot().contains(&addr));

        // An unhealthy teardown quarantines the address to prevent rapid re-selection.
        assert!(resolver.quarantined.contains_key(&addr));
    }

    #[test]
    fn handle_source_shutdown_when_healthy_releases_tracker_without_quarantine() {
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        // A benign teardown (e.g. pool resize or ctrl message): the source is still
        // Running when it shuts down.
        let in_use = InUseIpAddrs::new();
        in_use.add(addr).unwrap();
        let task_state = HashMap::from([(addr, State::Running)]);
        let mut resolver =
            make_resolver_with_in_use(task_state, HashMap::new(), None, in_use.clone());

        resolver.handle_ntp_task_shutdown(addr);

        // The task is gone: remove from task_state and release the global claim.
        assert!(!resolver.task_state.contains_key(&addr));
        assert!(!in_use.snapshot().contains(&addr));

        // A healthy source must NOT be quarantined, so it stays immediately eligible.
        assert!(!resolver.quarantined.contains_key(&addr));
    }

    #[test]
    fn two_resolvers_sharing_tracker_prevent_duplicate_selection() {
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        let shared_tracker = InUseIpAddrs::new();

        let mut resolver1 =
            make_resolver_with_in_use(HashMap::new(), HashMap::new(), None, shared_tracker.clone());
        let mut resolver2 =
            make_resolver_with_in_use(HashMap::new(), HashMap::new(), None, shared_tracker.clone());

        // First resolver claims the address
        let result1 = resolver1.new_addrs_to_launch(vec![addr], 5, true);
        assert_eq!(result1, vec![addr]);

        // Second resolver should NOT get the same address
        let result2 = resolver2.new_addrs_to_launch(vec![addr], 5, true);
        assert!(result2.is_empty());
    }

    #[test]
    fn pool_resolvers_share_global_in_use_tracker() {
        use crate::daemon::io::SourceState;
        use crate::daemon::io::dns::Pool;

        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        let shared_tracker = InUseIpAddrs::new();
        let (daemon_tx, _daemon_rx) = mpsc::channel(1);

        let mut pool1 = Pool::construct(
            "pool.ntp.org".to_string(),
            daemon_tx.clone(),
            shared_tracker.clone(),
            5,
        );
        let pool2 = Pool::construct(
            "pool.ntp.org".to_string(),
            daemon_tx,
            shared_tracker.clone(),
            5,
        );

        // Helper: snapshot the in-use addresses observed through a pool's resolver.
        let snapshot = |pool: &Pool| -> Vec<IpAddr> {
            let SourceState::Initialized(resolver) = &pool.resolver.state else {
                panic!("resolver should be Initialized immediately after construct");
            };
            resolver.in_use_ip_addrs.snapshot()
        };

        // Both pools start out observing an empty tracker.
        assert!(snapshot(&pool1).is_empty());
        assert!(snapshot(&pool2).is_empty());

        // Claim an address through pool1's resolver (the path a real source launch takes).
        // Because both pools were constructed with the same shared tracker, the claim must
        // be visible through pool2's resolver as well.
        let SourceState::Initialized(resolver1) = &mut pool1.resolver.state else {
            panic!("resolver should be Initialized immediately after construct");
        };
        let launched = resolver1.new_addrs_to_launch(vec![addr], 5, true);
        assert_eq!(launched, vec![addr]);

        assert!(snapshot(&pool1).contains(&addr));
        assert!(
            snapshot(&pool2).contains(&addr),
            "a source added via pool1 must be reflected in pool2's in-use snapshot"
        );

        // Removing the address through the shared tracker must likewise be reflected in
        // both pools' resolvers.
        shared_tracker.remove(&addr).unwrap();

        assert!(!snapshot(&pool1).contains(&addr));
        assert!(
            !snapshot(&pool2).contains(&addr),
            "a source removed from the shared tracker must be reflected in pool2's snapshot"
        );
    }
}
