//! Link Local test executable.
//!
//! This executable tests that the link local runner is able to send and receive packets from the
//! link local address and that the polling rate is roughly once a second.

use clock_bound::daemon::event::{Ntp, Stratum, ValidStratumLevel};
use clock_bound::daemon::io::{SourceIO, ntp::LINK_LOCAL_BURST_INTERVAL_DURATION};
use clock_bound::daemon::selected_clock::{ClockSource, SelectedClockSource};
use clock_bound::daemon::{
    async_ring_buffer::{self, BufferClosedError, Receiver},
    io::ntp::DaemonInfo,
};
use std::fs::OpenOptions;
use std::io::Seek;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time;

use rand::{RngCore, rng};
use tempfile::NamedTempFile;
use tokio::time::{Duration, timeout};
use tracing_subscriber::EnvFilter;

mod vmclock;
use vmclock::{VMClockContent, write_vmclock_content};

/// Time out for waiting on source polling task to produce an NTP event
///
/// On instances that aren't able to connect to link local the runner will run infinitely.
const TIMEOUT_SECS: u64 = 8; // Should yield 3 or more polls

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let (receiver, selected_clock, mut sourceio) = setup().await;
    test_startup_polling_rate(&receiver).await;
    test_normal_polling_rate(&receiver).await;
    test_clock_disruption_polling_rate(&receiver, &mut sourceio).await;
    test_polls_with_selected_clock_source_combos(&receiver, &selected_clock).await;
}

/// Set up link local source io for testing
async fn setup() -> (Receiver<Ntp>, Arc<SelectedClockSource>, SourceIO) {
    let (sender, receiver) = async_ring_buffer::create(1);

    let daemon_info = DaemonInfo {
        major_version: 2,
        minor_version: 100,
        startup_id: rng().next_u64(),
    };

    let selected_clock = Arc::new(SelectedClockSource::default());

    let mut sourceio = SourceIO::construct(selected_clock.clone(), daemon_info);
    sourceio.create_link_local(sender).await;
    sourceio.spawn_all();
    (receiver, selected_clock, sourceio)
}

/// Set up link local source io for testing
async fn setup_with_vmclock(vmclock_shm_path: &str, sourceio: &mut SourceIO) {
    sourceio.create_vmclock(vmclock_shm_path).await;
    sourceio.spawn_all();
}

/// Test normal polling
async fn test_normal_polling_rate(receiver: &Receiver<Ntp>) {
    println!("Testing normal polling rate ...");

    // Clear any previous packets from buffer
    while let Ok(Ok(_)) = timeout(Duration::from_millis(100), receiver.recv()).await {}

    // Poll the link local receiver
    let polling_rate = measure_polling_rate(receiver, Duration::from_secs(20)).await;

    println!("Polling rate avg: {polling_rate:?}");
    assert!(polling_rate.abs_diff(Duration::from_secs(2)) < time::Duration::from_millis(100));
    println!("Normal poll rate test PASSED");
}

/// Test startup polling
async fn test_startup_polling_rate(receiver: &Receiver<Ntp>) {
    println!("Testing startup polling rate ...");
    let polling_rate = measure_polling_rate(receiver, time::Duration::from_secs(1)).await;

    println!("Burst Polling rate avg: {polling_rate:?}");
    assert!(
        polling_rate.abs_diff(LINK_LOCAL_BURST_INTERVAL_DURATION)
            < time::Duration::from_millis(100)
    );
    println!("Burst poll rate test PASSED");
}

/// Test clock disruption polling
async fn test_clock_disruption_polling_rate(receiver: &Receiver<Ntp>, sourceio: &mut SourceIO) {
    println!("Testing normal polling rate ...");

    let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
    let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
    let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
    let mut vmclock_shm_file = OpenOptions::new()
        .write(true)
        .open(vmclock_shm_path)
        .expect("open vmclock file failed");
    let mut vmclock_content = VMClockContent::default();
    write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);

    setup_with_vmclock(vmclock_shm_path, sourceio).await;

    // Clear any previous packets from buffer
    while let Ok(Ok(_)) = timeout(Duration::from_millis(100), receiver.recv()).await {}

    // Base line polling rate.
    let pre_base_line_polling_rate = measure_polling_rate(receiver, Duration::from_secs(10)).await;
    println!("Base line Polling rate avg: {pre_base_line_polling_rate:?}");

    // Simulate a clock disruption event
    // Update the shared memory file
    vmclock_content.seq_count += 10;
    vmclock_content.disruption_marker += 1;

    let write_time = time::SystemTime::now();
    vmclock_shm_file.rewind().unwrap();
    write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);

    // Clock disruption polling rate
    #[allow(clippy::unchecked_time_subtraction)]
    let polling_duration = Duration::from_secs(1) - write_time.elapsed().unwrap();
    let clock_disruption_polling_rate = measure_polling_rate(receiver, polling_duration).await;
    println!("Clock disruption polling rate avg: {clock_disruption_polling_rate:?}");
    println!("Clock disruption polling duration: {polling_duration:?}");
    assert!(
        clock_disruption_polling_rate.abs_diff(LINK_LOCAL_BURST_INTERVAL_DURATION)
            < time::Duration::from_millis(100)
    );

    // Base line polling rate.
    let post_base_line_polling_rate = measure_polling_rate(receiver, Duration::from_secs(10)).await;
    println!("Base line Polling rate avg: {post_base_line_polling_rate:?}");

    // The pre and post clock disruption event polling rates should be the same.
    assert!(
        pre_base_line_polling_rate.abs_diff(post_base_line_polling_rate)
            < time::Duration::from_millis(100)
    );

    println!("Clock disruption rate test PASSED");
}

/// Measure the polling rate over a given period.
async fn measure_polling_rate(receiver: &Receiver<Ntp>, sampling_period: Duration) -> Duration {
    let start = time::Instant::now();
    let mut polling_rate = time::Duration::from_secs(0);
    let mut count = 0;
    loop {
        // On instances that aren't able to connect to link local the runner will run infinitely.
        // To address this we timeout if an NTP event has not been received.
        let lap_start = time::Instant::now();
        let ntpevent = timeout(Duration::from_secs(5), receiver.recv())
            .await
            .unwrap();
        let now = time::Instant::now();
        let d = now - lap_start;
        println!(
            "It looks like we got an ntp packet \n{ntpevent:#?}\n{:?} ms",
            d.as_millis()
        );

        // Skip the sample if it comes after the accepted duration.
        if start.elapsed() >= sampling_period {
            break;
        }

        // Skip the first sample, the IO runner will poll immediately after it's created.
        if count == 0 {
            count += 1;
        } else {
            polling_rate += d;
            count += 1;
        }
    }
    polling_rate /= count - 1;
    polling_rate
}

/// Test polling with varied selected clock source and stratum combinations
async fn test_polls_with_selected_clock_source_combos(
    receiver: &Receiver<Ntp>,
    selected_clock: &SelectedClockSource,
) {
    println!("Testing polling with all selected clock source combinations ...");

    let combinations = generate_selected_clock_combos();
    println!(
        "Generated {} selected clock source combinations",
        combinations.len()
    );

    for (source, stratum) in combinations {
        match source.clone() {
            ClockSource::Init => {} // Default state
            ClockSource::Phc => selected_clock.set_to_phc(),
            ClockSource::VMClock => selected_clock.set_to_vmclock(),
            ClockSource::None => selected_clock.set_to_none(),
            ClockSource::Server(ip) => {
                selected_clock.set_to_server(IpAddr::V4(Ipv4Addr::from(ip)), stratum);
            }
        }

        // Clear any stale events from the buffer
        while let Ok(Ok(_)) = timeout(Duration::from_millis(100), receiver.recv()).await {}

        println!("Polling with selected clock: {selected_clock}");
        #[allow(
            clippy::match_wild_err_arm,
            reason = "Only possible error from timeout is Elapsed, which has a private constructor"
        )]
        match timeout(Duration::from_secs(TIMEOUT_SECS), receiver.recv()).await {
            Ok(Ok(_ntp_event)) => {
                println!(" Polling succeeded.");
            }
            Ok(Err(BufferClosedError)) => {
                panic!(" Polling failed: Buffer closed");
            }
            Err(_) => {
                panic!(" Polling failed: Timed out. Selected clock: {selected_clock}");
            }
        }
    }

    println!("Poll test with all selected clock source combinations PASSED");
}

fn generate_selected_clock_combos() -> Vec<(ClockSource, Stratum)> {
    let mut combos = Vec::new();

    // Exhaustive match to fail compilation if new ClockSource variants are added
    let base_sources = [
        ClockSource::Init,
        ClockSource::Phc,
        ClockSource::VMClock,
        ClockSource::None,
        ClockSource::Server(0), // placeholder
    ];

    for source in base_sources {
        match source {
            ClockSource::Init => combos.push((ClockSource::Init, Stratum::Unspecified)),
            ClockSource::Phc => combos.push((ClockSource::Phc, Stratum::Unspecified)),
            ClockSource::VMClock => combos.push((ClockSource::VMClock, Stratum::Unspecified)),
            ClockSource::None => combos.push((ClockSource::None, Stratum::Unsynchronized)),
            ClockSource::Server(_) => {
                for level in 1..=15 {
                    let stratum = Stratum::Level(ValidStratumLevel::new(level).unwrap());
                    combos.push((
                        ClockSource::Server(u32::from_be_bytes([169, 254, 169, 123])),
                        stratum,
                    ));
                }
            }
        }
    }

    combos
}
