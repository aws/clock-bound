//! Clock Synchronization Daemon

#[cfg(feature = "algorithm")]
pub mod algo_analysis;

pub mod clock_parameters;

pub mod clock_sync_algorithm;

pub mod time;

pub mod event;

pub mod receiver_stream;

pub mod selected_clock;

#[cfg(feature = "daemon")]
pub mod autodetect;

#[cfg(feature = "daemon")]
pub mod source_mutator;

#[cfg(feature = "daemon")]
pub mod async_ring_buffer;

#[cfg(feature = "daemon")]
pub mod io;

#[cfg(feature = "daemon")]
pub mod clock_state;

#[cfg(feature = "daemon")]
pub mod config;

pub mod logging;

#[cfg(feature = "daemon")]
pub mod message;

// --- Everything below is only needed for the full daemon ---

#[cfg(feature = "daemon")]
use {
    crate::{
        daemon::{
            async_ring_buffer::Sender,
            autodetect::Autodetect,
            clock_state::ClockState,
            clock_sync_algorithm::{ClockSyncAlgorithm, Selector, SyncParameters},
            config::{Host, SourcesConfig},
            io::{ClockDisruptionEvent, ntp::DaemonInfo},
            message::Dns as DnsMessage,
            receiver_stream::{ReceiverStream, RoutableEvent},
            selected_clock::SelectedClockSource,
            source_mutator::SourceMutator,
            time::tsc::Skew,
        },
        shm::ClockStatus,
    },
    rand::{RngCore, rng},
    std::{net::SocketAddr, sync::Arc},
    tokio::sync::{mpsc, watch},
    tokio_util::{sync::CancellationToken, task::TaskTracker},
    tracing::{debug, error, warn},
};

/// The maximum dispersion growth every second
///
/// In between updates, the clock error bound continuously grows to take into account the worse
/// case drift of the underlying oscillator. The maximum dispersion is the rate of growth applied
/// to the last clock error bound update.
///
/// If the value is 15,000 parts per billion, for example, then every second we go without an
/// updated measurement the clock error bound will increase by 15 microseconds.
///
/// This number is based on CPU spec sheet error tolerances
#[cfg(feature = "daemon")]
pub(crate) const MAX_DISPERSION_GROWTH_PPB: u32 = 15_000;

#[cfg(feature = "daemon")]
const MAX_DISPERSION_GROWTH: Skew = Skew::from_ppb(MAX_DISPERSION_GROWTH_PPB as f64);

/// The number of messages that can be in the daemon's mpsc channel
///
/// There is no technical reason why this cannot be one. However, adding
/// more allows for less contention when initializing pools
#[cfg(feature = "daemon")]
const DAEMON_MESSAGE_CAPACITY: usize = 5;

#[cfg(feature = "daemon")]
pub struct Daemon {
    io_front_end: io::SourceIO,
    clock_sync_algorithm: ClockSyncAlgorithm,
    receiver_stream: ReceiverStream,
    clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
    cancellation_token: CancellationToken,
    clock_state_handle: ClockStateHandle,
    dns_message_rx: mpsc::Receiver<DnsMessage>,
    #[expect(
        dead_code,
        reason = "Only until configured sources are wired into source management"
    )]
    configured_sources: SourcesConfig,
}

#[cfg(feature = "daemon")]
impl Daemon {
    /// Construct and initialize a new daemon
    /// FIXME: Make this function not async. (Currently required for the io.run methods)
    pub async fn construct(
        configured_sources: SourcesConfig,
        cancellation_token: CancellationToken,
    ) -> Self {
        let daemon_info = DaemonInfo {
            major_version: 2,
            minor_version: 200,
            startup_id: rng().next_u64(),
        };

        let clock_state_cancellation_token = CancellationToken::new();

        let selected_clock = Arc::new(SelectedClockSource::default());

        // Initialize IO components.
        let (dns_message_tx, dns_message_rx) = mpsc::channel(DAEMON_MESSAGE_CAPACITY);
        let mut io_front_end =
            io::SourceIO::construct(selected_clock.clone(), daemon_info, dns_message_tx);
        let clock_disruption_receiver = io_front_end.clock_disruption_receiver();

        let mut receiver_stream = ReceiverStream::default();
        let mut clock_sync_algorithm = ClockSyncAlgorithm::builder()
            .selected_clock(selected_clock.clone())
            .selector(Selector::new(MAX_DISPERSION_GROWTH))
            .build();

        let mut source_mutator = SourceMutator::new(
            &mut io_front_end,
            &mut clock_sync_algorithm,
            &mut receiver_stream,
        );

        // Platform auto-detection: determine which sources to install.
        let autodetect_result = match Autodetect::detect() {
            Ok(result) => result,
            Err(e) => {
                warn!(
                    ?e,
                    "Platform auto-detection failed; defaulting to non-Amazon."
                );
                Autodetect::Other
            }
        };

        // Install Amazon-conditional sources.
        source_mutator
            .init_from_autodetect_results(&autodetect_result, MAX_DISPERSION_GROWTH)
            .await;

        // Install default pool
        source_mutator.add_pool(String::from("time.aws.com"));

        // Install sources from the configuration file.
        Self::install_configured_sources(&mut source_mutator, &configured_sources);

        // Read VMClock params (the source may have been created by init_amazon_sources above).
        let vmclock_params = io_front_end.vmclock_params();

        // Initialize the Clock State component
        let (clock_state_tx, clock_state) = {
            let (tx, rx) = async_ring_buffer::create(1);
            let clock_state = ClockState::construct(
                rx,
                clock_disruption_receiver.clone(),
                clock_state_cancellation_token.clone(),
                vmclock_params,
                ClockStatus::Unknown,
            );
            (tx, clock_state)
        };
        let task_tracker = TaskTracker::new();
        let clock_state_handle = ClockStateHandle {
            clock_state: Some(clock_state),
            tx: clock_state_tx,
            cancellation_token: clock_state_cancellation_token,
            task_tracker,
        };

        Self {
            io_front_end,
            clock_sync_algorithm,
            receiver_stream,
            clock_disruption_receiver,
            cancellation_token,
            clock_state_handle,
            dns_message_rx,
            configured_sources,
        }
    }

    /// Install the NTP sources from the configuration file into the daemon.
    ///
    /// Each configured source maps onto a `SourceMutator` action:
    /// - a `server` given as an IP is pinned as a fixed-address NTP source,
    /// - a `server` given as a name is added as a single-host DNS-backed source,
    /// - a `pool` is added as a multi-source DNS-backed pool.
    fn install_configured_sources(
        source_mutator: &mut SourceMutator<'_>,
        configured_sources: &SourcesConfig,
    ) {
        for configured_source in configured_sources.ntp() {
            match configured_source {
                config::NtpSource::Server(Host::Ip(ip)) => {
                    source_mutator.add_ntp_source(SocketAddr::new(*ip, 123), MAX_DISPERSION_GROWTH);
                }
                config::NtpSource::Server(Host::Domain(d)) => {
                    source_mutator.add_domain_host(d.to_string());
                }
                config::NtpSource::Pool(d) => {
                    source_mutator.add_pool(d.to_string());
                }
            }
        }
    }

    /// Run the daemon.
    pub async fn run(mut self: Box<Self>) {
        self.clock_sync_algorithm.init_repro();
        // Start IO polling
        self.io_front_end.spawn_all();
        self.clock_state_handle.task_tracker.spawn({
            #[expect(
                clippy::missing_panics_doc,
                reason = "struct always initialized with `Some`"
            )]
            let mut clock_state = self.clock_state_handle.clock_state.take().unwrap();
            async move {
                clock_state.run().await;
            }
        });
        self.clock_state_handle.task_tracker.close();
        loop {
            if self.run_once().await == RunLoopControl::Exit {
                break;
            }
        }
    }

    // Broken down into another fn to help with unit tests
    //
    // Allows custom test fixtures and assertion of [`Daemon`] state
    async fn run_once(&mut self) -> RunLoopControl {
        tokio::select! {
            biased; // biased to ensure disruption is handled first when this happens
            Ok(()) = self.clock_disruption_receiver.changed() => {
                self.handle_disruption();
                RunLoopControl::Continue
            }
            () = self.cancellation_token.cancelled() => {
                debug!("Received shutdown signal. Starting graceful shutdown of daemon.");

                // TODO: we can asynchronously shutdown both clock state and io tasks.

                // shutdown clock state task
                self.clock_state_handle.cancellation_token.cancel();
                self.clock_state_handle.task_tracker.wait().await;

                // shutdown all io tasks
                self.io_front_end.shutdown_all().await;

                // exit ourselves
                RunLoopControl::Exit
            }
            routable_event = self.receiver_stream.recv(), if !self.receiver_stream.is_empty() => {
                // TODO: This will return None if nothing is initialized. We need
                // the handle this scenario
                let routable_event = routable_event.unwrap();
                self.handle_event(routable_event);
                RunLoopControl::Continue
            }
            dns_message = self.dns_message_rx.recv() => {
                let dns_message = dns_message.unwrap();
                self.handle_dns_message(dns_message).await;
                RunLoopControl::Continue
            }
        }
    }

    fn handle_event(&mut self, routable_event: RoutableEvent) {
        if let Some(source_params) = self.clock_sync_algorithm.feed(routable_event) {
            use crate::daemon::async_ring_buffer::SendError;

            match self.clock_state_handle.tx.send(source_params.clone()) {
                Ok(()) => (),
                Err(SendError::Disrupted(source_params)) => {
                    // don't handle_disruption. It will be handled on the next call of tokio::select
                    debug!(
                        ?source_params,
                        "Trying to send a value when there was a disruption event. Dropping."
                    );
                }
                Err(SendError::BufferClosed(e)) => {
                    error!(
                        ?e,
                        "Trying to send a value when the buffer is closed. Panicking."
                    );
                    panic!("Unable to communicate with clock state. {e:?}");
                }
            }
        }
    }

    // Should only be called from the run fn
    async fn handle_dns_message(&mut self, dns_message: DnsMessage) {
        let mut source_mutator = SourceMutator::new(
            &mut self.io_front_end,
            &mut self.clock_sync_algorithm,
            &mut self.receiver_stream,
        );
        match dns_message {
            DnsMessage::AddPoolAddr(msg) => {
                source_mutator.add_pool_source(&msg.pool_domain, msg.addr, MAX_DISPERSION_GROWTH);
            }
            DnsMessage::RemovePoolAddr(msg) => {
                source_mutator
                    .remove_pool_source(&msg.pool_domain, msg.addr)
                    .await;
            }
        }
    }

    /// Handle a clock disruption event
    fn handle_disruption(&mut self) {
        // using destructure pattern ensure that all fields are handled
        let Self {
            io_front_end: _,
            clock_sync_algorithm,
            receiver_stream,
            clock_disruption_receiver,
            cancellation_token: _,
            clock_state_handle,
            dns_message_rx: _,
            configured_sources: _,
        } = self;

        let ClockStateHandle {
            clock_state: _,
            tx,
            cancellation_token: _,
            task_tracker: _,
        } = clock_state_handle;

        let val = clock_disruption_receiver.borrow_and_update().clone();
        if val.disruption_marker.is_some() {
            tx.handle_disruption();
            clock_sync_algorithm.handle_disruption();
            receiver_stream.handle_disruption();
        }
    }
}

#[cfg(feature = "daemon")]
struct ClockStateHandle {
    clock_state: Option<ClockState>,
    tx: Sender<SyncParameters>,
    cancellation_token: CancellationToken,
    task_tracker: TaskTracker,
}

/// Control flow signal for async run loops.
///
/// Used by tasks that call fallible operations mid-loop (e.g. sending on a
/// bounded channel) to communicate whether the loop should keep running or exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunLoopControl {
    /// The operation succeeded; continue the run loop.
    Continue,
    /// A shutdown signal was received; exit the run loop.
    Exit,
}

#[cfg(all(test, feature = "daemon"))]
mod tests {
    use std::{
        net::{IpAddr, Ipv4Addr, SocketAddr},
        str::FromStr,
    };

    use rstest::{fixture, rstest};
    use tokio::sync::mpsc;
    use tokio_util::{sync::CancellationToken, task::TaskTracker};

    use super::{DAEMON_MESSAGE_CAPACITY, Daemon, MAX_DISPERSION_GROWTH, RunLoopControl};
    use crate::daemon::{
        async_ring_buffer,
        clock_sync_algorithm::{ClockSyncAlgorithm, Selector},
        config::SourcesConfig,
        io::SourceIO,
        io::dns::resolver::{Message as ResolverMessage, Resolver},
        io::ntp::DaemonInfo,
        message::{AddPoolAddr, Dns as DnsMessage},
        receiver_stream::ReceiverStream,
        selected_clock::SelectedClockSource,
        source_mutator::SourceMutator,
    };
    use std::sync::Arc;

    const POOL_DOMAIN: &str = "time.aws.com";

    /// Harness bundling a [`Daemon`] with the channel endpoints a test needs to
    /// inject messages into (or observe) the daemon.
    ///
    /// The [`Daemon`] is built directly as a struct literal (bypassing the async,
    /// non-deterministic [`Daemon::construct`]) with `clock_state_handle.clock_state`
    /// set to `None`. No sources are installed by default; each test installs what it
    /// needs via [`SourceMutator`].
    struct TestDaemon {
        daemon: Daemon,
        /// Live clone of the daemon's DNS message sender.
        ///
        /// Retained so tests can inject [`DnsMessage`]s and so `dns_message_rx.recv()`
        /// in [`Daemon::run_once`] never spuriously returns `None`.
        dns_message_tx: mpsc::Sender<DnsMessage>,
    }

    /// Build a base [`Daemon`] with empty IO / clock sync algorithm / receiver stream,
    /// `clock_state: None`, a fresh (never-cancelled) cancellation token, and a live
    /// DNS message channel.
    #[fixture]
    fn test_daemon() -> TestDaemon {
        let selected_clock = Arc::new(SelectedClockSource::default());
        let daemon_info = DaemonInfo {
            major_version: 2,
            minor_version: 100,
            startup_id: 0xA_D00F_D00F_CAFE,
        };

        let (dns_message_tx, dns_message_rx) = mpsc::channel(DAEMON_MESSAGE_CAPACITY);
        let io_front_end =
            SourceIO::construct(selected_clock.clone(), daemon_info, dns_message_tx.clone());
        let clock_disruption_receiver = io_front_end.clock_disruption_receiver();

        let clock_sync_algorithm = ClockSyncAlgorithm::builder()
            .selected_clock(selected_clock)
            .selector(Selector::new(MAX_DISPERSION_GROWTH))
            .build();

        let receiver_stream = ReceiverStream::default();

        // Clock state is intentionally None: unit tests never run the clock state task.
        let (clock_state_tx, _clock_state_rx) = async_ring_buffer::create(1);
        let clock_state_handle = super::ClockStateHandle {
            clock_state: None,
            tx: clock_state_tx,
            cancellation_token: CancellationToken::new(),
            task_tracker: TaskTracker::new(),
        };

        let daemon = Daemon {
            io_front_end,
            clock_sync_algorithm,
            receiver_stream,
            clock_disruption_receiver,
            cancellation_token: CancellationToken::new(),
            clock_state_handle,
            dns_message_rx,
            configured_sources: SourcesConfig::default(),
        };

        TestDaemon {
            daemon,
            dns_message_tx,
        }
    }

    fn test_addr() -> SocketAddr {
        SocketAddr::from_str("192.0.2.1:123").unwrap()
    }

    /// A daemon with only a pool (no resolved address) has no time sources.
    ///
    /// Registering a pool wires nothing into the clock sync algorithm or receiver
    /// stream — those are only populated once a pool address is resolved.
    #[rstest]
    #[tokio::test]
    async fn pool_only_has_no_sources(test_daemon: TestDaemon) {
        let TestDaemon { mut daemon, .. } = test_daemon;

        {
            let mut mutator = SourceMutator::new(
                &mut daemon.io_front_end,
                &mut daemon.clock_sync_algorithm,
                &mut daemon.receiver_stream,
            );
            mutator.add_pool(POOL_DOMAIN.to_string());
        }

        // The pool exists in IO ...
        assert!(daemon.io_front_end.pools().contains_key(POOL_DOMAIN));
        assert_eq!(
            daemon
                .io_front_end
                .pools()
                .get(POOL_DOMAIN)
                .unwrap()
                .ntp_source_count(),
            0
        );

        // ... but no time sources are installed.
        assert!(daemon.receiver_stream.is_empty());
        assert!(daemon.clock_sync_algorithm.ntp_sources().is_empty());
        assert!(daemon.clock_sync_algorithm.amazon_time_sync().is_none());
        assert!(daemon.clock_sync_algorithm.phc().is_none());
    }

    /// A valid `AddPoolAddr` DNS message installs a pool source across all
    /// three daemon components.
    ///
    /// With only a pool registered, the receiver stream starts empty, so the guarded
    /// `receiver_stream.recv()` branch of `run_once` is inert. The DNS message is the
    /// sole ready branch, making the single `run_once` call deterministic.
    #[rstest]
    #[tokio::test]
    async fn add_pool_addr_installs_source(test_daemon: TestDaemon) {
        let TestDaemon {
            mut daemon,
            dns_message_tx,
        } = test_daemon;
        let addr = test_addr();

        {
            let mut mutator = SourceMutator::new(
                &mut daemon.io_front_end,
                &mut daemon.clock_sync_algorithm,
                &mut daemon.receiver_stream,
            );
            mutator.add_pool(POOL_DOMAIN.to_string());
        }

        // Pre-state: pool present, but no sources anywhere and no spawned tasks.
        assert!(daemon.receiver_stream.is_empty());
        assert!(daemon.clock_sync_algorithm.ntp_sources().is_empty());
        assert_eq!(
            daemon
                .io_front_end
                .pools()
                .get(POOL_DOMAIN)
                .unwrap()
                .ntp_source_count(),
            0
        );
        assert_eq!(daemon.io_front_end.task_count(), 0);

        // Inject a valid AddPoolAddr and drive a single run_once.
        dns_message_tx
            .send(DnsMessage::AddPoolAddr(AddPoolAddr {
                pool_domain: POOL_DOMAIN.to_string(),
                addr,
            }))
            .await
            .unwrap();
        assert_eq!(daemon.run_once().await, RunLoopControl::Continue);

        // ClockSyncAlgorithm: the source is present as a pool source.
        assert_eq!(daemon.clock_sync_algorithm.ntp_sources().len(), 1);
        assert_eq!(
            daemon.clock_sync_algorithm.ntp_sources()[0].socket_address(),
            addr
        );
        assert_eq!(
            daemon.clock_sync_algorithm.ntp_sources()[0].pool_domain(),
            Some(POOL_DOMAIN)
        );

        // ReceiverStream: the source's receiver is installed.
        assert_eq!(daemon.receiver_stream.len(), 1);
        assert!(daemon.receiver_stream.contains_ntp_source(&addr));

        // SourceIO: the pool now owns the source.
        assert_eq!(
            daemon
                .io_front_end
                .pools()
                .get(POOL_DOMAIN)
                .unwrap()
                .ntp_source_count(),
            1
        );

        // SourceIO: the NtpSource task was spawned onto the task tracker.
        assert_eq!(daemon.io_front_end.task_count(), 1);
    }

    /// An unreachable-address notification flows resolver -> daemon and tears
    /// the source down across all three daemon components.
    ///
    /// A standalone [`Resolver`] plays the role of the pool's resolver: it is wired to
    /// the daemon's DNS channel and pre-seeded with `addr` as `Running`. The test acts
    /// as the NTP IO task, sending [`ResolverMessage::UnreachableAddr`]. The resolver
    /// handles it synchronously (emitting `RemovePoolAddr` on the daemon channel), and a
    /// single `run_once` then processes the removal.
    #[rstest]
    #[tokio::test]
    async fn unreachable_addr_removes_source(test_daemon: TestDaemon) {
        let TestDaemon {
            mut daemon,
            dns_message_tx,
        } = test_daemon;
        let addr = test_addr();

        // Set up the daemon's pool with a live, removable source.
        {
            let mut mutator = SourceMutator::new(
                &mut daemon.io_front_end,
                &mut daemon.clock_sync_algorithm,
                &mut daemon.receiver_stream,
            );
            mutator.add_pool(POOL_DOMAIN.to_string());
            mutator.add_pool_source(POOL_DOMAIN, addr, MAX_DISPERSION_GROWTH);
        }

        // Sanity: the source is installed everywhere before removal.
        assert_eq!(daemon.clock_sync_algorithm.ntp_sources().len(), 1);
        assert!(daemon.receiver_stream.contains_ntp_source(&addr));
        assert_eq!(
            daemon
                .io_front_end
                .pools()
                .get(POOL_DOMAIN)
                .unwrap()
                .ntp_source_count(),
            1
        );

        // Standalone resolver wired to the daemon's DNS channel, pre-seeded with the addr.
        let (mut resolver, handles) =
            Resolver::new_running(POOL_DOMAIN.to_string(), addr.ip(), dns_message_tx);

        // Play the NTP IO task: report the address as unreachable.
        handles
            .resolver_message_tx
            .send(ResolverMessage::new_unreachable_addr(addr.ip()))
            .await
            .unwrap();

        // Resolver handles it synchronously, emitting RemovePoolAddr on the daemon channel.
        assert_eq!(
            resolver.recv_and_handle_one().await,
            RunLoopControl::Continue
        );

        // The daemon handles the resulting RemovePoolAddr.
        assert_eq!(daemon.run_once().await, RunLoopControl::Continue);

        // The source is gone from all three components.
        assert!(daemon.clock_sync_algorithm.ntp_sources().is_empty());
        assert!(!daemon.receiver_stream.contains_ntp_source(&addr));
        assert_eq!(daemon.receiver_stream.len(), 0);
        assert_eq!(
            daemon
                .io_front_end
                .pools()
                .get(POOL_DOMAIN)
                .unwrap()
                .ntp_source_count(),
            0
        );
    }

    /// Build a [`SourcesConfig`] from a TOML `[[sources.ntp]]` snippet.
    ///
    /// Goes through the real parse/validate path so the tests exercise the same
    /// `SourcesConfig` shape the daemon receives at startup.
    fn sources_from_toml(toml: &str) -> SourcesConfig {
        let config: crate::daemon::config::Config = ::toml::from_str(toml).expect("valid config");
        let (_logging, sources) = config.into_parts();
        sources
    }

    /// `install_configured_sources` with an empty config installs nothing.
    #[rstest]
    #[tokio::test]
    async fn install_configured_sources_empty_installs_nothing(test_daemon: TestDaemon) {
        let TestDaemon { mut daemon, .. } = test_daemon;
        let sources = SourcesConfig::default();

        {
            let mut mutator = SourceMutator::new(
                &mut daemon.io_front_end,
                &mut daemon.clock_sync_algorithm,
                &mut daemon.receiver_stream,
            );
            Daemon::install_configured_sources(&mut mutator, &sources);
        }

        assert!(daemon.io_front_end.pools().is_empty());
        assert!(daemon.clock_sync_algorithm.ntp_sources().is_empty());
        assert!(daemon.receiver_stream.is_empty());
    }

    /// A `server` given as an IP is installed as a fixed-address NTP source
    /// across the clock sync algorithm and receiver stream, on port 123.
    #[rstest]
    #[tokio::test]
    async fn install_configured_sources_ip_server(test_daemon: TestDaemon) {
        let TestDaemon { mut daemon, .. } = test_daemon;
        let sources = sources_from_toml(indoc::indoc! {r#"
            [[sources.ntp]]
            server = "192.0.2.1"
        "#});

        {
            let mut mutator = SourceMutator::new(
                &mut daemon.io_front_end,
                &mut daemon.clock_sync_algorithm,
                &mut daemon.receiver_stream,
            );
            Daemon::install_configured_sources(&mut mutator, &sources);
        }

        let expected = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 123);
        assert_eq!(daemon.clock_sync_algorithm.ntp_sources().len(), 1);
        assert_eq!(
            daemon.clock_sync_algorithm.ntp_sources()[0].socket_address(),
            expected
        );
        assert!(daemon.receiver_stream.contains_ntp_source(&expected));

        // A fixed-IP server is not a DNS-backed pool.
        assert!(daemon.io_front_end.pools().is_empty());
    }

    /// A `server` given as a name is installed as a single-host DNS-backed pool
    /// (capped at one source), touching only the IO layer until it resolves.
    #[rstest]
    #[tokio::test]
    async fn install_configured_sources_domain_server(test_daemon: TestDaemon) {
        let TestDaemon { mut daemon, .. } = test_daemon;
        let sources = sources_from_toml(indoc::indoc! {r#"
            [[sources.ntp]]
            server = "ntp.corp.example"
        "#});

        {
            let mut mutator = SourceMutator::new(
                &mut daemon.io_front_end,
                &mut daemon.clock_sync_algorithm,
                &mut daemon.receiver_stream,
            );
            Daemon::install_configured_sources(&mut mutator, &sources);
        }

        // Registered as a DNS-backed pool, no time source until resolution.
        assert!(daemon.io_front_end.pools().contains_key("ntp.corp.example"));
        assert!(daemon.clock_sync_algorithm.ntp_sources().is_empty());
        assert!(daemon.receiver_stream.is_empty());
    }

    /// A `pool` is installed as a DNS-backed pool, touching only the IO layer
    /// until it resolves.
    #[rstest]
    #[tokio::test]
    async fn install_configured_sources_pool(test_daemon: TestDaemon) {
        let TestDaemon { mut daemon, .. } = test_daemon;
        let sources = sources_from_toml(indoc::indoc! {r#"
            [[sources.ntp]]
            pool = "pool.ntp.example"
        "#});

        {
            let mut mutator = SourceMutator::new(
                &mut daemon.io_front_end,
                &mut daemon.clock_sync_algorithm,
                &mut daemon.receiver_stream,
            );
            Daemon::install_configured_sources(&mut mutator, &sources);
        }

        assert!(daemon.io_front_end.pools().contains_key("pool.ntp.example"));
        assert!(daemon.clock_sync_algorithm.ntp_sources().is_empty());
        assert!(daemon.receiver_stream.is_empty());
    }

    /// A mix of every source kind is installed together: the IP server lands as a
    /// fixed NTP source while the domain server and pool land as DNS-backed pools.
    #[rstest]
    #[tokio::test]
    async fn install_configured_sources_mixed(test_daemon: TestDaemon) {
        let TestDaemon { mut daemon, .. } = test_daemon;
        let sources = sources_from_toml(indoc::indoc! {r#"
            [[sources.ntp]]
            server = "192.0.2.1"

            [[sources.ntp]]
            server = "ntp.corp.example"

            [[sources.ntp]]
            pool = "pool.ntp.example"
        "#});

        {
            let mut mutator = SourceMutator::new(
                &mut daemon.io_front_end,
                &mut daemon.clock_sync_algorithm,
                &mut daemon.receiver_stream,
            );
            Daemon::install_configured_sources(&mut mutator, &sources);
        }

        // The IP server is a fixed-address NTP source.
        let expected = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 123);
        assert_eq!(daemon.clock_sync_algorithm.ntp_sources().len(), 1);
        assert_eq!(
            daemon.clock_sync_algorithm.ntp_sources()[0].socket_address(),
            expected
        );
        assert!(daemon.receiver_stream.contains_ntp_source(&expected));

        // The domain server and the pool are both DNS-backed pools.
        assert!(daemon.io_front_end.pools().contains_key("ntp.corp.example"));
        assert!(daemon.io_front_end.pools().contains_key("pool.ntp.example"));
        assert_eq!(daemon.io_front_end.pools().len(), 2);
    }
}
