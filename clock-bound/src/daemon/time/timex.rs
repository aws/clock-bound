use std::ops::{Deref, DerefMut};

use bon::bon;
use libc::{
    MOD_FREQUENCY, MOD_NANO, MOD_OFFSET, MOD_STATUS, MOD_TIMECONST, STA_FREQHOLD, STA_PLL, timeval,
    timex,
};
use tracing::warn;

use crate::daemon::time::{Duration, tsc::Skew};

const MAX_PHASE_OFFSET: Duration = Duration::from_millis(500);
const MAX_SKEW: Skew = Skew::from_ppm(512.0);

/// Newtype wrapping `libc::timex` to provide valid
/// constructor with less verbosity. Provides a constructor `new`
/// which zero-initializes all fields.
#[derive(Debug)]
pub struct Timex(timex);

#[bon]
impl Timex {
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
        let mut tx = Self::default();
        if skew > MAX_SKEW || skew < -MAX_SKEW {
            warn!("Skew of {skew} is outside of bounds +/-{MAX_SKEW}, clamping the value",);
            skew = skew.clamp(-MAX_SKEW, MAX_SKEW);
        }
        tx.freq = skew.to_timex_freq();
        if phase_correction > MAX_PHASE_OFFSET || phase_correction < -MAX_PHASE_OFFSET {
            warn!(
                "Phase correction of {}ns is outside of bounds +/-{}ns, clamping the value",
                phase_correction.as_nanos(),
                MAX_PHASE_OFFSET.as_nanos()
            );
            phase_correction = phase_correction.clamp(-MAX_PHASE_OFFSET, MAX_PHASE_OFFSET);
        }
        tx.offset = phase_correction.as_nanos() as i64;

        // PLL clock adjustment proportion is dependent on this time constant.
        // The clock adjustment factor over the length of a second
        // is calculated as `shift_right(offset, SHIFT_PLL + ntpdata->time_constant)`, where const `SHIFT_PLL` = 2
        // So, if we want to correct the clock quickly, we use a lower time constant.
        // The value is clamped between 0 and 10.
        // For now, we use 0, to aggressively correct the clock, which means we'd expect for
        // offset to be corrected by `offset >> 2` every second (exponentially decaying)
        tx.constant = 0;
        // Set `modes` bits for all fields we modify, plus ADJ_NANO to use nanosecond units
        // and ADJ_STATUS to set status bits below.
        tx.modes = MOD_FREQUENCY | MOD_OFFSET | MOD_TIMECONST | MOD_NANO | MOD_STATUS;
        // STA_FREQHOLD: Hold the frequency that we prescribe, if this is omitted the PLL would modify `freq`
        // which we do not want since ClockBound's clock sync algorithm should determine the proper
        // frequency setting.
        // STA_PLL: Additionally, only rely on PLL to perform phase adjustments
        tx.status = STA_FREQHOLD | STA_PLL;

        tx
    }
}

impl Default for Timex {
    fn default() -> Self {
        Self(timex {
            modes: 0,
            offset: 0,
            freq: 0,
            maxerror: 0,
            esterror: 0,
            status: 0,
            constant: 0,
            precision: 0,
            tolerance: 0,
            time: timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
            tick: 0,
            ppsfreq: 0,
            jitter: 0,
            shift: 0,
            stabil: 0,
            jitcnt: 0,
            calcnt: 0,
            errcnt: 0,
            stbcnt: 0,
            tai: 0,
            __unused1: 0,
            __unused2: 0,
            __unused3: 0,
            __unused4: 0,
            __unused5: 0,
            __unused6: 0,
            __unused7: 0,
            __unused8: 0,
            __unused9: 0,
            __unused10: 0,
            __unused11: 0,
        })
    }
}

impl Deref for Timex {
    type Target = timex;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Timex {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
#[cfg(test)]
mod test {
    use rstest::rstest;

    use super::*;

    #[test]
    fn test_timex_default() {
        let timex = Timex::default();
        assert_eq!(timex.modes, 0);
        assert_eq!(timex.offset, 0);
        assert_eq!(timex.freq, 0);
        assert_eq!(timex.maxerror, 0);
        assert_eq!(timex.esterror, 0);
        assert_eq!(timex.status, 0);
        assert_eq!(timex.constant, 0);
        assert_eq!(timex.precision, 0);
        assert_eq!(timex.tolerance, 0);
        assert_eq!(timex.time.tv_sec, 0);
        assert_eq!(timex.time.tv_usec, 0);
        assert_eq!(timex.tick, 0);
        assert_eq!(timex.ppsfreq, 0);
        assert_eq!(timex.jitter, 0);
        assert_eq!(timex.shift, 0);
        assert_eq!(timex.stabil, 0);
        assert_eq!(timex.jitcnt, 0);
        assert_eq!(timex.calcnt, 0);
        assert_eq!(timex.errcnt, 0);
        assert_eq!(timex.stbcnt, 0);
        assert_eq!(timex.tai, 0);
        assert_eq!(timex.__unused1, 0);
        assert_eq!(timex.__unused2, 0);
        assert_eq!(timex.__unused3, 0);
        assert_eq!(timex.__unused4, 0);
        assert_eq!(timex.__unused5, 0);
        assert_eq!(timex.__unused6, 0);
        assert_eq!(timex.__unused7, 0);
        assert_eq!(timex.__unused8, 0);
        assert_eq!(timex.__unused9, 0);
        assert_eq!(timex.__unused10, 0);
        assert_eq!(timex.__unused11, 0);
    }

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
        let tx = Timex::clock_adjustment()
            .phase_correction(phase_correction)
            .skew(skew)
            .call();
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
}
