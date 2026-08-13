//! A program to step `CLOCK_REALTIME` by a given phase correction using the
//! timekeeping utilities internal to ClockBound.
use chrono::{DateTime, Utc};
use clap::Parser;
use clock_bound::daemon::{
    clock_state::clock_adjust::{KAPIClockAdjuster, NtpAdjTimeExt},
    time::Duration,
};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    /// Phase correction to apply to the clock in seconds (as a floating point number).
    #[arg(short, long, allow_negative_numbers = true)]
    phase_correction_seconds: f64,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let phase_correction = Duration::from_seconds_f64(args.phase_correction_seconds);

    let initial_time: DateTime<Utc> = Utc::now();
    println!("Initial time is {initial_time:?}");
    let clock_adjuster = KAPIClockAdjuster;
    clock_adjuster
        .apply_phase_correction_step(phase_correction)
        .map_err(|e| anyhow::anyhow!(e))?;
    println!(
        "Applied {:+} second phase correction to kernel to step CLOCK_REALTIME.",
        phase_correction.as_seconds_f64(),
    );
    let final_time: DateTime<Utc> = Utc::now();
    println!("Final time is {final_time:?}");
    Ok(())
}
