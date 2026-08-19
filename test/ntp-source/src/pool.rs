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

use clock_bound::daemon::clock_sync_algorithm::{ClockSyncAlgorithm, Selector};
use clock_bound::daemon::io::SourceIO;
use clock_bound::daemon::io::ntp::DaemonInfo;
use clock_bound::daemon::message::Dns as DnsMessage;
use clock_bound::daemon::receiver_stream::{ReceiverStream, RoutableEvent};
use clock_bound::daemon::selected_clock::SelectedClockSource;
use clock_bound::daemon::source_mutator::SourceMutator;
use clock_bound::daemon::time::tsc::Skew;

use rand::{RngCore, rng};
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

/// Pool domain to test against — same as the daemon uses in production.
const POOL_DOMAIN: &str = "time.aws.com";

/// Maximum time to wait before declaring the test failed.
const TIMEOUT: Duration = Duration::from_secs(90);

/// Expected number of sources the resolver will fill the pool to.
/// Matches `MAX_SOURCES` in `clock_bound::daemon::io::dns::resolver`.
const EXPECTED_SOURCES: usize = 5;

/// Minimum number of NTP responses we require from each created source
/// before declaring the test successful.
const MIN_RESPONSES_PER_SOURCE: usize = 2;

/// Maximum oscillator dispersion growth (15 ppm), same as the daemon.
const MAX_DISPERSION_GROWTH: Skew = Skew::from_ppm(15.0);

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    test_pool_source_creation_lifecycle().await;
}

/// Components needed by the test run loop.
struct TestHarness {
    io: SourceIO,
    csa: ClockSyncAlgorithm,
    receiver_stream: ReceiverStream,
    dns_message_rx: mpsc::Receiver<DnsMessage>,
}

/// Construct all components and wire up the pool.
fn setup() -> TestHarness {
    let selected_clock = Arc::new(SelectedClockSource::default());
    let daemon_info = DaemonInfo {
        major_version: 2,
        minor_version: 100,
        startup_id: rng().next_u64(),
    };

    let (dns_message_tx, dns_message_rx) = mpsc::channel::<DnsMessage>(16);

    let mut io = SourceIO::construct(selected_clock.clone(), daemon_info, dns_message_tx);
    let mut csa = ClockSyncAlgorithm::builder()
        .selected_clock(selected_clock.clone())
        .selector(Selector::new(MAX_DISPERSION_GROWTH))
        .build();
    let mut receiver_stream = ReceiverStream::default();

    // Add the pool
    SourceMutator::new(&mut io, &mut csa, &mut receiver_stream).add_pool(POOL_DOMAIN.to_string());

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

    let mut harness = setup();
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
                handle_dns_message(msg, &mut harness, &mut responses_per_source).await;
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

/// The pool is ready once every expected source has been created and each
/// created source has produced at least [`MIN_RESPONSES_PER_SOURCE`] responses.
fn pool_ready(responses_per_source: &HashMap<SocketAddr, usize>) -> bool {
    responses_per_source.len() >= EXPECTED_SOURCES
        && responses_per_source
            .values()
            .all(|count| *count >= MIN_RESPONSES_PER_SOURCE)
}

/// Process a `DnsMessage` from the resolver, forwarding it through the `SourceMutator`.
async fn handle_dns_message(
    msg: Option<DnsMessage>,
    harness: &mut TestHarness,
    responses_per_source: &mut HashMap<SocketAddr, usize>,
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
                EXPECTED_SOURCES,
            );
            let mut mutator = SourceMutator::new(
                &mut harness.io,
                &mut harness.csa,
                &mut harness.receiver_stream,
            );
            mutator.add_pool_source(&add.pool_domain, add.addr, MAX_DISPERSION_GROWTH);
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
