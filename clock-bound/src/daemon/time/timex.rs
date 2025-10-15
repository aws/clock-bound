//! This module contains a newtype `Timex` wrapping an inner `libc::timex`, which allows construction
//! only of valid values for the sake of the types of `adjtimex`/`ntp_adjtime` calls we'll make in ClockBound.
use bon::bon;
#[cfg(not(target_os = "macos"))]
use libc::timeval;
use libc::{
    ADJ_SETOFFSET, MOD_FREQUENCY, MOD_NANO, MOD_OFFSET, MOD_STATUS, MOD_TIMECONST, STA_FREQHOLD,
    STA_PLL, timex,
};
use tracing::warn;

use crate::daemon::time::{Duration, tsc::Skew};

const MAX_PHASE_OFFSET: Duration = Duration::from_millis(500);
const MAX_SKEW: Skew = Skew::from_ppm(512.0);

/// Newtype wrapping `libc::timex` to provide valid
/// constructors for each type of `adjtimex`/`ntp_adjtime` operation.
#[derive(Debug)]
pub struct Timex(timex);

#[bon]
impl Timex {
    /// Expose a mutable reference to the inner `libc::timex`.
    pub fn expose(&mut self) -> *mut timex {
        &raw mut self.0
    }

    /// Builds a `libc::timex` used for adjustment of the system clock, to apply the given phase correction
    /// and skew values, in a single system call.
    ///
    /// The skew, or frequency error relative to a baseline at 0, is applied directly
    /// to the `freq` in the timekeeping utilities, e.g. passing +1ppm will "speed up" the clock
    /// by 1ppm. Thus, if the clock is slow by 1 microsecond every second, we should pass in +1ppm.
    ///
    /// The phase correction will be passed directly to the kernel PLL to correct the system
    /// clock via a slew with exponential decaying effect (the proportion % corrected per second
    /// is controlled by `tx.constant`).
    ///
    /// NOTE:
    /// Slew correction of the offset by PLL is NOT applied to `freq`, but rather to the `tick_length` used in the kernel
    /// which is used to calculate the `mult` factor used in timestamping (e.g. `ns ~= (clocksource * mult) >> shift`)
    /// This allows our `freq` control via `skew` parameter to be independent of the phase correction, for better stability.
    /// Caveat of this, is that if we are still slewing, we might our estimate of the phase offset between `CLOCK_REALTIME` and
    /// ClockBound's internal clock could be prone to error/overestimates, so controlling how/when we estimate that offset is
    /// needed.
    #[builder]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "phase correction is clamped then converted so no truncation"
    )]
    pub fn clock_adjustment(mut phase_correction: Duration, mut skew: Skew) -> Self {
        if skew > MAX_SKEW || skew < -MAX_SKEW {
            warn!("Skew of {skew} is outside of bounds +/-{MAX_SKEW}, clamping the value",);
            skew = skew.clamp(-MAX_SKEW, MAX_SKEW);
        }
        if phase_correction > MAX_PHASE_OFFSET || phase_correction < -MAX_PHASE_OFFSET {
            warn!(
                "Phase correction of {}ns is outside of bounds +/-{}ns, clamping the value",
                phase_correction.as_nanos(),
                MAX_PHASE_OFFSET.as_nanos()
            );
            phase_correction = phase_correction.clamp(-MAX_PHASE_OFFSET, MAX_PHASE_OFFSET);
        }
        Self(timex {
            // Set `modes` bits for all fields we modify, plus ADJ_NANO to use nanosecond units
            // and ADJ_STATUS to set status bits below.
            modes: MOD_FREQUENCY | MOD_OFFSET | MOD_TIMECONST | MOD_NANO | MOD_STATUS,
            offset: phase_correction.as_nanos() as i64,
            freq: skew.to_timex_freq(),
            maxerror: 0,
            esterror: 0,
            // STA_FREQHOLD: Hold the frequency that we prescribe, if this is omitted the PLL would modify `freq`
            // which we do not want since ClockBound's clock sync algorithm should determine the proper
            // frequency setting.
            // STA_PLL: Additionally, only rely on PLL to perform phase adjustments
            status: STA_FREQHOLD | STA_PLL,
            // PLL clock adjustment proportion is dependent on this time constant.
            // The clock adjustment factor over the length of a second
            // is calculated as `shift_right(offset, SHIFT_PLL + ntpdata->time_constant)`, where const `SHIFT_PLL` = 2
            // So, if we want to correct the clock quickly, we use a lower time constant.
            // The value is clamped between 0 and 10.
            // For now, we use 0, to aggressively correct the clock, which means we'd expect for
            // offset to be corrected by `offset >> 2` every second (exponentially decaying)
            constant: 0,
            precision: 0,
            tolerance: 0,
            #[cfg(not(target_os = "macos"))]
            time: timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
            #[cfg(not(target_os = "macos"))]
            tick: 0,
            ppsfreq: 0,
            jitter: 0,
            shift: 0,
            stabil: 0,
            jitcnt: 0,
            calcnt: 0,
            errcnt: 0,
            stbcnt: 0,
            #[cfg(not(target_os = "macos"))]
            tai: 0,
            #[cfg(not(target_os = "macos"))]
            __unused1: 0,
            #[cfg(not(target_os = "macos"))]
            __unused2: 0,
            #[cfg(not(target_os = "macos"))]
            __unused3: 0,
            #[cfg(not(target_os = "macos"))]
            __unused4: 0,
            #[cfg(not(target_os = "macos"))]
            __unused5: 0,
            #[cfg(not(target_os = "macos"))]
            __unused6: 0,
            #[cfg(not(target_os = "macos"))]
            __unused7: 0,
            #[cfg(not(target_os = "macos"))]
            __unused8: 0,
            #[cfg(not(target_os = "macos"))]
            __unused9: 0,
            #[cfg(not(target_os = "macos"))]
            __unused10: 0,
            #[cfg(not(target_os = "macos"))]
            __unused11: 0,
        })
    }

    /// Construct a `libc::timex` used for stepping the clock by some phase correction,
    /// with a full step (can go forwards or backwards).
    /// This is used to set the system clock to the current time, which is useful for
    /// initializing the clock after a reboot.
    ///
    /// `ADJ_SETOFFSET` is only supported on Linux `adjtimex`, in the future we should have some implementation
    /// for other platforms e.g. FreeBSD
    #[cfg(target_os = "linux")]
    #[allow(
        clippy::field_reassign_with_default,
        reason = "false positive, can't use default constructor for inner type fields mutated"
    )]
    #[builder]
    pub fn clock_step(phase_correction: Duration) -> Self {
        Self(timex {
            // Set `modes` bits for `ADJ_SETOFFSET` to step the clock, and MOD_NANO to use nanosecond units
            modes: ADJ_SETOFFSET | MOD_NANO,
            offset: 0,
            freq: 0,
            maxerror: 0,
            esterror: 0,
            status: 0,
            constant: 0,
            precision: 0,
            tolerance: 0,
            // `ADJ_SETOFFSET` uses `time` rather than offset field to indicate how much to step the clock
            #[cfg(not(target_os = "macos"))]
            time: phase_correction.to_timeval_nanos(),
            #[cfg(not(target_os = "macos"))]
            tick: 0,
            ppsfreq: 0,
            jitter: 0,
            shift: 0,
            stabil: 0,
            jitcnt: 0,
            calcnt: 0,
            errcnt: 0,
            stbcnt: 0,
            #[cfg(not(target_os = "macos"))]
            tai: 0,
            #[cfg(not(target_os = "macos"))]
            __unused1: 0,
            #[cfg(not(target_os = "macos"))]
            __unused2: 0,
            #[cfg(not(target_os = "macos"))]
            __unused3: 0,
            #[cfg(not(target_os = "macos"))]
            __unused4: 0,
            #[cfg(not(target_os = "macos"))]
            __unused5: 0,
            #[cfg(not(target_os = "macos"))]
            __unused6: 0,
            #[cfg(not(target_os = "macos"))]
            __unused7: 0,
            #[cfg(not(target_os = "macos"))]
            __unused8: 0,
            #[cfg(not(target_os = "macos"))]
            __unused9: 0,
            #[cfg(not(target_os = "macos"))]
            __unused10: 0,
            #[cfg(not(target_os = "macos"))]
            __unused11: 0,
        })
    }
}

impl AsRef<timex> for Timex {
    fn as_ref(&self) -> &timex {
        &self.0
    }
}

#[cfg(test)]
mod test {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::upper_bound(
        Duration::from_millis(500),
        Skew::from_ppm(512.0),
        500_000_000,
        33_554_432
    )]
    #[case::above_upper_bound_gets_clamped(
        Duration::from_millis(600),
        Skew::from_ppm(1024.0),
        500_000_000,
        33_554_432
    )]
    #[case::lower_bound(
        Duration::from_millis(-500),
        Skew::from_ppm(-512.0),
        -500_000_000,
        -33_554_432,
    )]
    #[case::below_lower_bound_gets_clamped(
        Duration::from_millis(-600),
        Skew::from_ppm(-1024.0),
        -500_000_000,
        -33_554_432,
    )]
    #[case::normal_value_positives(
        Duration::from_millis(100),
        Skew::from_ppm(128.0),
        100_000_000,
        8_388_608
    )]
    #[case::normal_value_negatives(
        Duration::from_millis(-100),
        Skew::from_ppm(-128.0),
        -100_000_000,
        -8_388_608,
    )]
    #[case::zero(Duration::from_millis(0), Skew::from_ppm(0.0), 0, 0)]
    fn test_timex_clock_adjustment(
        #[case] phase_correction: Duration,
        #[case] skew: Skew,
        #[case] expected_offset: i64,
        #[case] expected_freq: i64,
    ) {
        let binding = Timex::clock_adjustment()
            .phase_correction(phase_correction)
            .skew(skew)
            .call();
        let tx = binding.as_ref();
        assert_eq!(tx.offset, expected_offset);
        assert_eq!(tx.freq, expected_freq);
        // assert modes, status and constant are set properly for our adjustment
        assert_eq!(
            tx.modes,
            MOD_FREQUENCY | MOD_OFFSET | MOD_TIMECONST | MOD_NANO | MOD_STATUS
        );
        assert_eq!(tx.status, STA_FREQHOLD | STA_PLL);
        assert_eq!(tx.constant, 0);
    }

    #[rstest]
    #[case::positive(
        Duration::from_millis(100),
        timeval {tv_sec: 0, tv_usec: 100_000_000},
    )]
    #[case::negative(
        -Duration::from_millis(100),
        timeval {tv_sec: -1, tv_usec: 900_000_000},
    )]
    #[case::zero(
        Duration::from_millis(0),
        timeval {tv_sec: 0, tv_usec: 0},
    )]
    fn test_timex_clock_step(#[case] phase_correction: Duration, #[case] expected_time: timeval) {
        let binding = Timex::clock_step()
            .phase_correction(phase_correction)
            .call();
        let tx = binding.as_ref();
        assert_eq!(tx.time, expected_time);
        // assert modes is set properly for our adjustment
        assert_eq!(tx.modes, ADJ_SETOFFSET | MOD_NANO);
    }
}
