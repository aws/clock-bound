//! CI Integration Test: ClockBound SHM Error Bound Validation
//!
//! Validates that `CLOCK_REALTIME` (disciplined by chrony in our CI environment) falls within
//! the error bounds advertised by ClockBound via shared memory.
//!
//! ClockBound is compiled with the `test-side-by-side` feature so it does NOT modify
//! the system clock. Chrony continues to discipline `CLOCK_REALTIME` independently.
//!
//! Note: The test reads `CLOCK_REALTIME` directly and does not depend on chrony
//! specifically — in practice, the test can run beside any NTP client that disciplines the system clock.

mod results;
mod types;

use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use clock_bound::client::ClockBoundClient;
use nix::time::{ClockId, clock_gettime};

use results::{TestResult, ValidationReport};
use types::{BoundCheck, RawSample};

/// Grace period at startup before we start collecting samples (seconds).
/// Gives clockbound time to converge after startup.
const WARMUP_SECS: u64 = 10;

/// Command-line arguments for the validation test.
#[derive(Debug, Parser)]
#[command(
    name = "shmv3-error-bound-validation",
    about = "Validate ClockBound SHM bounds vs system clock"
)]
struct Args {
    /// Duration of the sampling loop in seconds (excludes warmup).
    #[clap(long)]
    sampling_loop_duration: u64,

    /// Path to the ClockBound shared memory segment.
    #[clap(long, default_value = "/var/run/clockbound/shm1")]
    shm_path: String,

    /// Warmup period in seconds. During warmup, no samples are collected.
    #[clap(long, default_value_t = WARMUP_SECS)]
    warmup_secs: u64,

    /// Path to write the JSON results artifact.
    #[clap(long, default_value = "bound_check_results.json")]
    output_path: String,

    /// Interval between samples in microseconds. Controls the sampling rate
    /// to prevent excessive memory usage during long test runs.
    #[clap(long, default_value_t = 500)]
    sample_interval_micros: u64,
}

/// Output of the measurement loop.
struct CollectionResult {
    samples: Vec<RawSample>,
    /// Number of iterations where one or both SHM reads failed.
    shm_errors: u64,
}

/// Attempt to connect to the ClockBound SHM segment, retrying every 500ms
/// until `timeout` is reached. In CI, the SHM segment is created asynchronously
/// by the clockbound daemon after the `CodeBuild` instance starts, so a retry window is needed.
fn connect_shm_client(shm_path: &str, timeout: Duration) -> Result<ClockBoundClient, String> {
    let start = Instant::now();
    loop {
        match ClockBoundClient::new_with_path(shm_path) {
            Ok(c) => {
                println!(
                    "ClockBound SHM client connected (took {:.1}s)",
                    start.elapsed().as_secs_f64()
                );
                return Ok(c);
            }
            Err(e) => {
                let elapsed = start.elapsed();
                if elapsed >= timeout {
                    return Err(format!(
                        "Failed to open ClockBound SHM at '{}' after {}s: {e:#?}",
                        shm_path,
                        timeout.as_secs()
                    ));
                }
                thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

/// Run the measurement loop for `duration` and collect raw samples.
///
/// The loop reads the ClockBound SHM before and after reading `CLOCK_REALTIME`,
/// producing a pair of bound windows that bracket the realtime measurement.
///
/// Samples are taken at `sample_interval` apart to control memory usage.
fn collect_samples(
    client: &mut ClockBoundClient,
    duration: Duration,
    sample_interval: Duration,
) -> CollectionResult {
    let start = Instant::now();
    let deadline = start + duration;
    let mut samples: Vec<RawSample> = Vec::new();
    let mut shm_errors: u64 = 0;

    println!("Starting measurement loop...");

    // Hot loop: capture raw measurements at the configured interval.
    while Instant::now() < deadline {
        let cb_before = client.now();
        let realtime = clock_gettime(ClockId::CLOCK_REALTIME).expect("clock_gettime failed");
        let cb_after = client.now();

        if let (Ok(before), Ok(after)) = (cb_before, cb_after) {
            samples.push(RawSample {
                elapsed_secs: start.elapsed().as_secs_f64(),
                realtime,
                before,
                after,
            });
        } else {
            shm_errors += 1;
        }

        thread::sleep(sample_interval);
    }

    println!(
        "Measurement complete. Collected {} samples ({} SHM errors).",
        samples.len(),
        shm_errors
    );
    CollectionResult {
        samples,
        shm_errors,
    }
}

/// Transform raw samples into validated `BoundCheck`s and aggregate into `ValidationReport`.
fn validate(samples: &[RawSample], shm_errors: u64) -> ValidationReport {
    let checks: Vec<BoundCheck> = samples.iter().map(BoundCheck::from_sample).collect();
    ValidationReport::from_checks(checks, shm_errors)
}

fn main() {
    let args = Args::parse();

    println!("=== ClockBound SHM Error Bound Validation ===");
    println!("Sampling loop duration: {}s", args.sampling_loop_duration);
    println!("Warmup period: {}s", args.warmup_secs);
    println!("Sample interval: {}μs", args.sample_interval_micros);
    println!("SHM path: {}", args.shm_path);
    println!();

    let mut client = match connect_shm_client(&args.shm_path, Duration::from_secs(20)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        }
    };

    println!("Warming up for {}s...", args.warmup_secs);
    thread::sleep(Duration::from_secs(args.warmup_secs));

    let sampling_duration = Duration::from_secs(args.sampling_loop_duration);
    let sample_interval = Duration::from_micros(args.sample_interval_micros);
    let collection = collect_samples(&mut client, sampling_duration, sample_interval);

    let report = validate(&collection.samples, collection.shm_errors);

    report.write_json(&args.output_path);
    report.print_summary();

    match &report.result {
        TestResult::Passed => {
            println!(
                "PASSED: All {} combined assertions held.",
                report.successful_combined_assertions
            );
        }
        TestResult::NoSamplesCollected => {
            eprintln!("FAILED: No samples collected. ClockBound SHM may not be responding.");
            std::process::exit(1);
        }
        TestResult::CombinedBoundAssertionFailure(_) => {
            eprintln!("FAILED: System clock escaped ClockBound combined error bounds.");
            std::process::exit(1);
        }
        TestResult::ClockStateInvalid(_) => {
            eprintln!("FAILED: >10% of samples had invalid clock state.");
            std::process::exit(1);
        }
    }
}
