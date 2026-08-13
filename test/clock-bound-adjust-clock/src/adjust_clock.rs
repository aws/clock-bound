//! A program to apply a clock phase correction and skew correction using
//! the timekeeping utilities internal to ClockBound.
use clap::Parser;
use clock_bound::daemon::{
    clock_state::clock_adjust::{KAPIClockAdjuster, NtpAdjTimeExt},
    time::Duration,
};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    /// Phase correction to apply to the clock in seconds (as a floating point number).
    /// The offset is fed into the kernel PLL, which slews the clock and also derives a
    /// frequency correction over time.
    #[arg(short, long, allow_negative_numbers = true)]
    phase_correction_seconds: f64,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let phase_correction = Duration::from_seconds_f64(args.phase_correction_seconds);

    let clock_adjuster = KAPIClockAdjuster;
    clock_adjuster
        .apply_phase_correction_smooth(phase_correction)
        .map_err(|e| anyhow::anyhow!(e))?;
    println!(
        "Applied {:+} second phase correction to kernel PLL (smooth slew).",
        phase_correction.as_seconds_f64(),
    );
    Ok(())
}
