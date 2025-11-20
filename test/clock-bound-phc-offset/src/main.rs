//! PHC offset test executable.
//!
//! This executable compares specific clocks against timestamps read from the PHC.

use anyhow::Result;
use clap::{Parser, ValueEnum};

use clock_bound::daemon::time::clocks::{MonotonicRaw, RealTime};
use clock_bound_client_generic::{ClockBoundClient, ClockBoundClientVersion};
use clock_bound_phc_offset::autoconfigure_phc_reader;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ClockToCompare {
    RealTime,
    MonotonicRaw,
    ClockBoundClientV2,
    ClockBoundClientV3,
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(value_enum, long)]
    clock: ClockToCompare,
}

fn emit_logs<T: serde::Serialize>(clock: ClockToCompare, result: Result<T>) {
    match result {
        Ok(data) => println!("{}", serde_json::to_string(&data).unwrap()),
        Err(e) => tracing::error!(
            "Failed to retrieve data from {clock:?} Clock <-> PHC comparison: {e:?}"
        ),
    }
}

fn main() {
    tracing_subscriber::fmt().json().init();
    let args = Args::parse();
    let clock = args.clock;

    let phc_reader =
        autoconfigure_phc_reader().expect("failed to create PHC reader via autoconfiguration");

    match clock {
        ClockToCompare::RealTime => emit_logs(
            ClockToCompare::RealTime,
            phc_reader.get_offset_from_utc_clock(&RealTime),
        ),
        ClockToCompare::MonotonicRaw => emit_logs(
            ClockToCompare::MonotonicRaw,
            phc_reader.get_offset_from_utc_clock(&MonotonicRaw),
        ),
        ClockToCompare::ClockBoundClientV2 => {
            let mut client = ClockBoundClient::new(ClockBoundClientVersion::V2);
            emit_logs(
                ClockToCompare::ClockBoundClientV2,
                phc_reader.compare_to_clock_bound_client_clock(&mut client),
            );
        }
        ClockToCompare::ClockBoundClientV3 => {
            let mut client = ClockBoundClient::new(ClockBoundClientVersion::V3);
            emit_logs(
                ClockToCompare::ClockBoundClientV2,
                phc_reader.compare_to_clock_bound_client_clock(&mut client),
            );
        }
    }
}
