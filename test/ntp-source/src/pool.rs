//! DNS pool IO-loop integration test.
//!
//! # What this tests
//!
//! This is an end-to-end exercise of the daemon's **IO loop** for DNS-backed
//! NTP pools. It stands up the same components the daemon uses in production
//! (`SourceIO`, `ClockSyncAlgorithm`, `ReceiverStream`, and the `SourceMutator`
//! that wires them together) and drives them from a single-task
//! `tokio::select!` that mirrors the daemon's own run loop. The loop has two
//! branches, exactly as the daemon does:
//!
//! - It receives `DnsMessage` commands from the `Resolver` and applies them via
//!   the `SourceMutator` to add or remove pool sources dynamically.
//! - It consumes NTP events produced by those sources through the
//!   `ReceiverStream`.
//!
//! The full path under test is therefore: real DNS resolution of the pool
//! domain by the `Resolver` → dynamic source spawning in response to
//! `AddPoolAddr` messages → live NTP requests to the resolved servers → NTP
//! events flowing back out through the `ReceiverStream`. Because it uses the
//! real resolver and real network sources, it validates that these pieces are
//! wired together correctly and that the IO loop keeps servicing both branches
//! concurrently (adding sources while events stream in) without stalling.
//!
//! # What it means to pass
//!
//! The test tracks every source IP the resolver creates and counts the NTP
//! responses received from each one. It passes only when the IO loop has:
//!
//! 1. created at least `EXPECTED_SOURCES` (5) distinct pool sources, and
//! 2. delivered at least `MIN_RESPONSES_PER_SOURCE` (2) NTP responses from
//!    *each* created source.
//!
//! Requiring multiple responses per source (rather than a single event
//! anywhere) proves the loop sustains bidirectional traffic to every source it
//! spawned, not just to a lucky few. If either condition is not met within
//! `TIMEOUT`, the test panics with the per-source response counts so the
//! failing sources are obvious.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use clock_bound::daemon::async_ring_buffer;
use clock_bound::daemon::clock_sync_algorithm::{ClockSyncAlgorithm, FfSyncParams, Selector};
use clock_bound::daemon::event;
use clock_bound::daemon::io::dns::ntp_source::NtpSource;
use clock_bound::daemon::io::dns::resolver;
use clock_bound::daemon::io::ntp::DaemonInfo;
use clock_bound::daemon::io::{ClockDisruptionEvent, ControlRequest, SourceIO};
use clock_bound::daemon::message::Dns as DnsMessage;
use clock_bound::daemon::receiver_stream::{ReceiverStream, RoutableEvent};
use clock_bound::daemon::selected_clock::SelectedClockSource;
use clock_bound::daemon::source_mutator::SourceMutator;

use clap::Parser;
use rand::{RngCore, rng};
use tokio::sync::{mpsc, watch};
use tracing_subscriber::EnvFilter;

/// Pool domain to test against — same as the daemon uses in production.
const POOL_DOMAIN: &str = "time.aws.com";

/// Maximum time to wait before declaring the test failed.
const TIMEOUT: Duration = Duration::from_secs(90);

/// Expected number of sources the resolver will fill the pool to.
/// Matches the count `SourceMutator::add_pool` forwards to the resolver.
const EXPECTED_SOURCES: usize = 5;

/// Expected number of sources for a single-host domain.
/// Matches the count `SourceMutator::add_domain_host` forwards to the resolver.
const EXPECTED_SINGLE_HOST_SOURCES: usize = 1;

/// Minimum number of NTP responses we require from each created source
/// before declaring the test successful.
const MIN_RESPONSES_PER_SOURCE: usize = 2;

/// Command-line options for the pool test binary.
#[derive(Debug, Parser)]
struct Args {
    /// Run the IPv6 source test.
    ///
    /// Use on hosts with working IPv6 connectivity
    #[clap(long)]
    run_ipv6_tests: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    test_pool_source_creation_lifecycle().await;
    test_single_host_lifecycle().await;
    if args.run_ipv6_tests {
        test_ipv6_source_sends_events().await;
    } else {
        println!("Skipping IPv6 tests (no --run-ipv6-tests flag)");
    }
}

/// Components needed by the test run loop.
struct TestHarness {
    io: SourceIO,
    csa: ClockSyncAlgorithm,
    receiver_stream: ReceiverStream,
    dns_message_rx: mpsc::Receiver<DnsMessage>,
}

/// How the pool domain is installed into the harness.
#[derive(Clone, Copy)]
enum PoolKind {
    /// Multi-source pool via `SourceMutator::add_pool`.
    Pool,
    /// Single-host domain via `SourceMutator::add_domain_host`.
    SingleHost,
}

/// Construct all components and wire up the pool.
fn setup(kind: PoolKind) -> TestHarness {
    let selected_clock = Arc::new(SelectedClockSource::default());
    let daemon_info = DaemonInfo {
        major_version: 2,
        minor_version: 200,
        startup_id: rng().next_u64(),
    };

    let (dns_message_tx, dns_message_rx) = mpsc::channel::<DnsMessage>(16);

    let mut io = SourceIO::construct(selected_clock.clone(), daemon_info, dns_message_tx);
    let ff_sync_params = FfSyncParams::default();
    let mut csa = ClockSyncAlgorithm::builder()
        .selected_clock(selected_clock.clone())
        .selector(Selector::new(ff_sync_params.max_dispersion))
        .build();
    let mut receiver_stream = ReceiverStream::default();

    // Install the pool domain via the requested mutator entry point.
    let mut mutator = SourceMutator::new(&mut io, &mut csa, &mut receiver_stream);
    match kind {
        PoolKind::Pool => mutator.add_pool(POOL_DOMAIN.to_string()),
        PoolKind::SingleHost => mutator.add_domain_host(POOL_DOMAIN.to_string()),
    }

    TestHarness {
        io,
        csa,
        receiver_stream,
        dns_message_rx,
    }
}

/// Test the full pool lifecycle: DNS resolution → source spawning → NTP events.
async fn test_pool_source_creation_lifecycle() {
    println!("Testing DNS pool lifecycle with domain: {POOL_DOMAIN}");

    println!(
        "config: pool_domain={POOL_DOMAIN}, timeout={TIMEOUT:?}, \
         expected_sources={EXPECTED_SOURCES}, min_responses_per_source={MIN_RESPONSES_PER_SOURCE}"
    );

    let mut harness = setup(PoolKind::Pool);
    harness.io.spawn_all();

    println!("Pool created and resolver spawned. Waiting for DNS resolution and NTP events...");

    // Track every created source IP and the number of NTP responses seen from
    // it. A source is inserted with a count of 0 when it is added to the pool,
    // then incremented each time an NTP event arrives from that address.
    let mut responses_per_source: HashMap<SocketAddr, usize> = HashMap::new();
    let deadline = tokio::time::Instant::now() + TIMEOUT;

    // Periodic heartbeat so a stall is visible before the timeout fires.
    let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
    heartbeat.tick().await; // consume the immediate first tick

    loop {
        if pool_ready(&responses_per_source) {
            break;
        }

        // Only poll the receiver stream once at least one source exists,
        // otherwise recv() returns None immediately (empty stream) and
        // starves the dns_message_rx branch in a hot loop.
        let have_sources = !responses_per_source.is_empty();
        tokio::select! {
            msg = harness.dns_message_rx.recv() => {
                handle_dns_message(msg, &mut harness, &mut responses_per_source, EXPECTED_SOURCES).await;
            }
            event = harness.receiver_stream.recv(), if have_sources => {
                handle_event(event, &mut responses_per_source);
            }
            _ = heartbeat.tick() => {
                let satisfied = responses_per_source
                    .values()
                    .filter(|c| **c >= MIN_RESPONSES_PER_SOURCE)
                    .count();
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                println!(
                    "  progress: sources={}/{EXPECTED_SOURCES}, satisfied={satisfied}, \
                     time_left={remaining:?}, counts={responses_per_source:?}",
                    responses_per_source.len(),
                );
            }
            () = tokio::time::sleep_until(deadline) => {
                panic!(
                    "TIMEOUT after {TIMEOUT:?}. \
                     sources_added={}/{EXPECTED_SOURCES}, \
                     responses_per_source={responses_per_source:?}, \
                     require >= {MIN_RESPONSES_PER_SOURCE} responses each",
                    responses_per_source.len(),
                );
            }
        }
    }

    let sources_added = responses_per_source.len();
    assert!(
        sources_added >= EXPECTED_SOURCES,
        "Expected at least {EXPECTED_SOURCES} sources, got {sources_added}"
    );
    let deficient: Vec<_> = responses_per_source
        .iter()
        .filter(|(_, count)| **count < MIN_RESPONSES_PER_SOURCE)
        .collect();
    assert!(
        deficient.is_empty(),
        "Sources below {MIN_RESPONSES_PER_SOURCE} responses: {deficient:?}"
    );

    println!("DNS pool test PASSED");
    println!("  Sources added: {sources_added}");
    let total_responses: usize = responses_per_source.values().sum();
    println!("  NTP events received: {total_responses}");
    for (addr, count) in &responses_per_source {
        println!("    {addr}: {count} responses");
    }

    harness.io.shutdown_all().await;
}

/// Test the single-host path: `add_domain_host` caps the pool at one live source.
///
/// Unlike the multi-source pool test, this asserts the resolver never drives the
/// pool above a single concurrent source, and that the one source produces at
/// least [`MIN_RESPONSES_PER_SOURCE`] NTP responses. This validates the
/// `max_sources = 1` plumbing end-to-end rather than just that a source works.
async fn test_single_host_lifecycle() {
    println!("Testing single-host domain lifecycle with domain: {POOL_DOMAIN}");

    let mut harness = setup(PoolKind::SingleHost);
    harness.io.spawn_all();

    println!("Single-host domain created and resolver spawned. Waiting for NTP events...");

    // Currently live sources (added and not yet removed) and their response counts.
    let mut responses_per_source: HashMap<SocketAddr, usize> = HashMap::new();
    // Highest number of concurrently-live sources observed. Must never exceed 1.
    let mut max_concurrent_sources: usize = 0;
    let deadline = tokio::time::Instant::now() + TIMEOUT;

    let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
    heartbeat.tick().await; // consume the immediate first tick

    loop {
        // Ready once the single live source has enough responses.
        if responses_per_source
            .values()
            .any(|count| *count >= MIN_RESPONSES_PER_SOURCE)
        {
            break;
        }

        let have_sources = !responses_per_source.is_empty();
        tokio::select! {
            msg = harness.dns_message_rx.recv() => {
                handle_dns_message(
                    msg,
                    &mut harness,
                    &mut responses_per_source,
                    EXPECTED_SINGLE_HOST_SOURCES,
                )
                .await;
                let live = responses_per_source.len();
                max_concurrent_sources = max_concurrent_sources.max(live);
                assert!(
                    max_concurrent_sources <= 1,
                    "single-host domain exceeded one concurrent source: live={live}, \
                     sources={responses_per_source:?}"
                );
            }
            event = harness.receiver_stream.recv(), if have_sources => {
                handle_event(event, &mut responses_per_source);
            }
            _ = heartbeat.tick() => {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                println!(
                    "  progress: live_sources={}, max_concurrent={max_concurrent_sources}, \
                     time_left={remaining:?}, counts={responses_per_source:?}",
                    responses_per_source.len(),
                );
            }
            () = tokio::time::sleep_until(deadline) => {
                panic!(
                    "TIMEOUT after {TIMEOUT:?}. max_concurrent={max_concurrent_sources}, \
                     responses_per_source={responses_per_source:?}, \
                     require >= {MIN_RESPONSES_PER_SOURCE} responses from the single source",
                );
            }
        }
    }

    assert!(
        max_concurrent_sources <= 1,
        "single-host domain exceeded one concurrent source (max observed: {max_concurrent_sources})"
    );

    println!("Single-host domain test PASSED");
    println!("  Max concurrent sources: {max_concurrent_sources}");
    for (addr, count) in &responses_per_source {
        println!("    {addr}: {count} responses");
    }

    harness.io.shutdown_all().await;
}

/// The pool is ready once every expected source has been created and each
/// created source has produced at least [`MIN_RESPONSES_PER_SOURCE`] responses.
fn pool_ready(responses_per_source: &HashMap<SocketAddr, usize>) -> bool {
    responses_per_source.len() >= EXPECTED_SOURCES
        && responses_per_source
            .values()
            .all(|count| *count >= MIN_RESPONSES_PER_SOURCE)
}

/// Process a `DnsMessage` from the resolver, forwarding it through the `SourceMutator`.
///
/// `expected_sources` is only used to annotate the `AddPoolAddr` log line with a
/// `current/target` count; it does not affect behavior.
async fn handle_dns_message(
    msg: Option<DnsMessage>,
    harness: &mut TestHarness,
    responses_per_source: &mut HashMap<SocketAddr, usize>,
    expected_sources: usize,
) {
    let Some(msg) = msg else {
        panic!(
            "DNS message channel closed unexpectedly after {} sources added: {responses_per_source:?}",
            responses_per_source.len(),
        );
    };
    match msg {
        DnsMessage::AddPoolAddr(add) => {
            println!(
                "  AddPoolAddr: {} (source {}/{})",
                add.addr,
                responses_per_source.len() + 1,
                expected_sources,
            );
            let mut mutator = SourceMutator::new(
                &mut harness.io,
                &mut harness.csa,
                &mut harness.receiver_stream,
            );
            let ff_sync_params = FfSyncParams::default();
            mutator.add_pool_source(&add.pool_domain, add.addr, &ff_sync_params);
            // Register the created IP so we can track its response count. Use
            // `entry` so a re-added address does not reset an existing count.
            responses_per_source.entry(add.addr).or_insert(0);
        }
        DnsMessage::RemovePoolAddr(remove) => {
            let prior = responses_per_source.get(&remove.addr).copied().unwrap_or(0);
            println!(
                "  RemovePoolAddr: {} (had {prior} responses before removal)",
                remove.addr
            );
            let mut mutator = SourceMutator::new(
                &mut harness.io,
                &mut harness.csa,
                &mut harness.receiver_stream,
            );
            mutator
                .remove_pool_source(&remove.pool_domain, remove.addr)
                .await;
            responses_per_source.remove(&remove.addr);
        }
    }
}

/// Process a `RoutableEvent` from the `ReceiverStream`.
fn handle_event(
    event: Option<RoutableEvent>,
    responses_per_source: &mut HashMap<SocketAddr, usize>,
) {
    match event {
        Some(RoutableEvent::NtpSource(addr, _ntp)) => {
            // Count this response against the source that produced it. The
            // source should already be tracked from its `AddPoolAddr`, but use
            // `entry` defensively in case an event races ahead of the add.
            let count = responses_per_source.entry(addr).or_insert(0);
            *count += 1;
            if *count == 1 {
                println!("  first NTP response from {addr}");
            }
            println!("  NTP event received from pool source {addr} [{count} from this source]");
        }
        Some(other) => {
            println!("  Unexpected event: {other:?}");
        }
        None => {
            println!("  ReceiverStream returned None (no streams available yet)");
        }
    }
}

/// Number of NTP events to require from the IPv6 source before the test passes.
const IPV6_REQUIRED_EVENTS: usize = 2;

/// Directly exercise a DNS NTP source pointed at a resolved **IPv6** address.
///
/// Unlike the other tests in this file, this one bypasses the resolver/`SourceIO`
/// machinery and drives a single [`NtpSource`] itself. Its purpose is narrow: prove
/// that when a pool domain resolves to an IPv6 address, an `NtpSource` constructed for
/// that `SocketAddr::V6` actually binds an IPv6 socket, sends NTP requests, and streams
/// responses back out through the ring buffer. This guards the family-aware bind in
/// `Ntp::sample`: with an IPv4-only bind, every send to a v6 destination would fail and
/// no events would ever arrive here.
///
/// This test runs on a runner that is expected to have working IPv6, so a missing AAAA
/// answer or absent IPv6 egress is a hard failure rather than a skip. It resolves
/// `time.aws.com`, picks the first IPv6 answer, verifies the host can actually route to it,
/// and then requires NTP events to flow. Failing loudly on these preconditions ensures the
/// v6 send path is genuinely exercised and a regression cannot hide behind a silent skip.
async fn test_ipv6_source_sends_events() {
    println!("Testing IPv6 DNS NTP source with domain: {POOL_DOMAIN}");

    let ipv6_addr = resolve_first_ipv6(POOL_DOMAIN).await.unwrap_or_else(|| {
        panic!("{POOL_DOMAIN} did not resolve to any IPv6 address; IPv6 is required here");
    });

    let server_address = SocketAddr::new(ipv6_addr.into(), 123);
    assert!(server_address.is_ipv6(), "expected an IPv6 socket address");
    println!("  resolved IPv6 source: {server_address}");

    // A AAAA record does not imply the host can route to it. Probe real IPv6 egress and
    // fail if there is none: this runner is expected to have working IPv6, so an
    // unreachable v6 destination is a genuine failure, not an environment quirk to skip.
    assert!(
        ipv6_reachable(ipv6_addr).await,
        "host has no working IPv6 route to {server_address}; \
         IPv6 egress is required to exercise the IPv6 send path"
    );
    println!("  verified IPv6 egress to {server_address}");

    // Build the same channels `SourceIO` would wire up for a DNS NTP source, but keep the
    // handles here so we can drive the source directly.
    let (event_sender, event_receiver) = async_ring_buffer::create::<event::Ntp>(16);
    let (_ctrl_sender, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
    let (_disruption_sender, disruption_receiver) =
        watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent::default());
    // The source registers/notifies the resolver over this channel. We stand in for the
    // resolver by draining it so the source's sends never block.
    let (resolver_tx, mut resolver_rx) = mpsc::channel::<resolver::Message>(16);

    let selected_clock = Arc::new(SelectedClockSource::default());
    let daemon_info = DaemonInfo {
        major_version: 2,
        minor_version: 200,
        startup_id: rng().next_u64(),
    };

    let mut source = NtpSource::construct(
        POOL_DOMAIN.to_string(),
        server_address,
        event_sender,
        ctrl_receiver,
        disruption_receiver,
        selected_clock,
        daemon_info,
        resolver_tx,
    );

    // Spawn the resolver-side drain so `register_with_resolver` and any later
    // notifications from the source never fill the channel and stall `run`.
    let resolver_drain = tokio::spawn(async move {
        while let Some(msg) = resolver_rx.recv().await {
            println!("  resolver received: {msg:?}");
        }
    });

    // Run the source itself. `run` loops until shutdown; the test only needs it alive
    // long enough to produce events, so it is aborted once enough events arrive.
    let source_task = tokio::spawn(async move {
        if let Err(error) = source.run().await {
            println!("  source run exited with error: {error:?}");
        }
    });

    // Collect events from the ring buffer, requiring more than one so we know the source
    // sustains the send/receive cycle over IPv6 rather than getting a single lucky reply.
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    let mut events_received = 0usize;
    while events_received < IPV6_REQUIRED_EVENTS {
        tokio::select! {
            result = event_receiver.recv() => {
                match result {
                    Ok(ntp) => {
                        events_received += 1;
                        println!(
                            "  IPv6 NTP event {events_received}/{IPV6_REQUIRED_EVENTS} \
                             from {server_address}: {ntp:?}"
                        );
                    }
                    Err(error) => {
                        panic!("ring buffer closed before enough IPv6 events: {error:?}");
                    }
                }
            }
            () = tokio::time::sleep_until(deadline) => {
                panic!(
                    "TIMEOUT after {TIMEOUT:?} waiting for IPv6 NTP events from {server_address}. \
                     received {events_received}/{IPV6_REQUIRED_EVENTS}"
                );
            }
        }
    }

    source_task.abort();
    resolver_drain.abort();

    println!("IPv6 DNS NTP source test PASSED");
    println!("  IPv6 source {server_address} produced {events_received} events");
}

/// Resolve `domain:123` and return the first IPv6 address in the answer, if any.
async fn resolve_first_ipv6(domain: &str) -> Option<std::net::Ipv6Addr> {
    let mut addrs = tokio::net::lookup_host((domain, 123))
        .await
        .unwrap_or_else(|error| panic!("DNS resolution of {domain} failed: {error:?}"));

    addrs.find_map(|addr| match addr {
        SocketAddr::V6(v6) => Some(*v6.ip()),
        SocketAddr::V4(_) => None,
    })
}

/// Probe whether this host can actually route to the given IPv6 address.
///
/// Mirrors the daemon's own `verify_ipv6` check: bind a local IPv6 wildcard socket and
/// `connect` it to the destination. For UDP this performs no network round trip; it only
/// sets the peer and lets the kernel confirm a usable route exists, so it fails fast on a
/// host with no global IPv6 address or default route.
async fn ipv6_reachable(server_addr: std::net::Ipv6Addr) -> bool {
    let socket = match tokio::net::UdpSocket::bind((std::net::Ipv6Addr::UNSPECIFIED, 0)).await {
        Ok(socket) => socket,
        Err(error) => {
            println!("  IPv6 bind failed during reachability probe: {error:?}");
            return false;
        }
    };
    match socket.connect((server_addr, 123)).await {
        Ok(()) => true,
        Err(error) => {
            println!("  IPv6 connect failed during reachability probe: {error:?}");
            false
        }
    }
}
