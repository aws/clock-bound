//! PHC offset test executable.
//!
//! This executable compares specific clocks against timestamps read from the PHC.

use clap::{Parser, ValueEnum};

use clock_bound::daemon::time::clocks::{MonotonicRaw, RealTime};
use clock_bound_phc_offset::autoconfigure_phc_reader;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ClockToCompare {
    RealTime,
    MonotonicRaw,
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(value_enum, long)]
    clock: ClockToCompare,
}

fn main() {
    tracing_subscriber::fmt().json().init();
    let args = Args::parse();
    let clock = args.clock;

    let phc_reader =
        autoconfigure_phc_reader().expect("failed to create PHC reader via autoconfiguration");

    let offset_rtt_ceb = match clock {
        ClockToCompare::RealTime => phc_reader.get_offset_from_utc_clock(&RealTime),
        ClockToCompare::MonotonicRaw => phc_reader.get_offset_from_utc_clock(&MonotonicRaw),
    };

    match offset_rtt_ceb {
        Ok(offset_rtt_ceb) => tracing::info!("{offset_rtt_ceb:?}"),
        Err(e) => tracing::error!(
            "Failed to retrieve offset, rtt, ceb from {clock:?} Clock <-> PHC comparison: {e:?}"
        ),
    }
}
