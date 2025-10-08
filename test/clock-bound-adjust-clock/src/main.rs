//! A program to apply a clock phase correction and skew correction using
//! the timekeeping utilities internal to ClockBound.
use clap::Parser;
use clock_bound::daemon::{
    clock_state::{ClockAdjuster, KAPIClockAdjuster},
    time::{Duration, tsc::Skew},
};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    /// Skew adjustment value, translated to a frequency
    /// adjustment in the kernel. Given a positive value, a
    /// positive `freq` value will be applied in the kernel timing
    /// parameters, which would "speed up" `CLOCK_REALTIME`, and vice versa
    /// for negative values.
    #[arg(short, long, allow_negative_numbers = true)]
    skew_ppb: i64,

    /// Phase correction to apply to the clock in seconds (as a floating point number).
    #[arg(short, long, allow_negative_numbers = true)]
    phase_correction_seconds: f64,
}

#[allow(
    clippy::cast_precision_loss,
    reason = "skew ends up being clamped anyways so no loss of precision in i64 -> f64"
)]
fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let phase_correction = Duration::from_seconds_f64(args.phase_correction_seconds);
    let skew = Skew::from_ppm(args.skew_ppb as f64 * 1e-3);

    let clock_adjuster = ClockAdjuster::new(KAPIClockAdjuster);
    clock_adjuster
        .adjust_clock(phase_correction, skew)
        .map_err(|e| anyhow::anyhow!(e))?;
    println!(
        "Applied {:+} frequency setting ({:+} ppb) and {:+} second phase correction to kernel to slew/correct.",
        skew.to_timex_freq(),
        skew.get() * 1e9,
        phase_correction.as_seconds_f64(),
    );
    Ok(())
}
