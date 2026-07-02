//! This module contains a newtype `Timex` wrapping an inner `libc::timex`, which allows construction
//! only of valid values for the sake of the types of `adjtimex`/`ntp_adjtime` calls we'll make in ClockBound.
use bon::bon;
#[cfg(not(target_os = "macos"))]
use libc::timeval;
use libc::{
    ADJ_SETOFFSET, MOD_FREQUENCY, MOD_NANO, MOD_OFFSET, MOD_STATUS, MOD_TIMECONST, STA_FREQHOLD,
    STA_NANO, STA_PLL, timex,
};
use tracing::warn;

use crate::daemon::time::{Duration, Instant, tsc::Skew};

const MAX_PHASE_OFFSET: Duration = Duration::from_millis(500);
const MAX_SKEW: Skew = Skew::from_ppm(512.0);

/// Newtype wrapping `libc::timex` to provide valid
/// constructors for each type of `adjtimex`/`ntp_adjtime` operation.
#[derive(Debug, PartialEq, Clone)]
pub struct Timex(timex);

#[bon]
impl Timex {
    /// Expose a mutable reference to the inner `libc::timex`.
    pub fn expose(&mut self) -> *mut timex {
        &raw mut self.0
    }

    /// Read the given `time` from the underlying `timex`. The kernel
    /// generally returns this with the time set to the current `CLOCK_REALTIME`
    /// reading.
    ///
    /// Notably, this same field is used in `ADJ_SETOFFSET` mode calls in order to supply an
    /// offset value.
    ///
    /// The value may be expressed in microseconds by default, but if the call is made
    /// with `status` bit `STA_NANO` set, `tv_usec` represents a nanosecond value.
    pub fn time(&self) -> Instant {
        let tv = self.0.time;
        let fractional_part = if self.0.status & STA_NANO > 0 {
            Duration::from_nanos(tv.tv_usec)
        } else {
            Duration::from_micros(tv.tv_usec)
        };
        Instant::from_secs(tv.tv_sec) + fractional_part
    }

    /// Reads the given `freq` from the underlying `timex`.
    ///
    /// In struct timex, freq, ppsfreq, and stabil are ppm (parts per
    /// million) with a 16-bit fractional part, which means that a value
    /// of 1 in one of those fields actually means 2^-16 ppm, and
    /// 2^16=65536 is 1 ppm.  This is the case for both input values (in
    /// the case of freq) and output values.
    /// ref: See NOTES in <https://man7.org/linux/man-pages/man2/adjtimex.2.html>
    ///
    /// This function constructs a `Skew` value from the given `freq` value set on the `timex`.
    pub fn freq(&self) -> Skew {
        Skew::from_timex_freq(self.0.freq)
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
            offset: phase_correction.as_nanos_trunc(),
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

    #[allow(
        clippy::cast_possible_truncation,
        reason = "phase correction is clamped then converted so no truncation"
    )]
    pub fn phase_correction(mut phase_correction: Duration) -> Self {
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
            modes: MOD_OFFSET | MOD_TIMECONST | MOD_NANO | MOD_STATUS,
            offset: phase_correction.as_nanos_trunc(),
            freq: 0,
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

    pub fn frequency_correction(mut skew: Skew) -> Self {
        if skew > MAX_SKEW || skew < -MAX_SKEW {
            warn!("Skew of {skew} is outside of bounds +/-{MAX_SKEW}, clamping the value",);
            skew = skew.clamp(-MAX_SKEW, MAX_SKEW);
        }
        Self(timex {
            // Set `modes` bits for all fields we modify, plus ADJ_NANO to use nanosecond units
            // and ADJ_STATUS to set status bits below.
            modes: MOD_FREQUENCY | MOD_NANO | MOD_STATUS,
            offset: 0,
            freq: skew.to_timex_freq(),
            maxerror: 0,
            esterror: 0,
            // STA_FREQHOLD: Hold the frequency that we prescribe, if this is omitted the PLL would modify `freq`
            // which we do not want since ClockBound's clock sync algorithm should determine the proper
            // frequency setting.
            // STA_PLL: Additionally, only rely on PLL to perform phase adjustments
            status: STA_FREQHOLD | STA_PLL,
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

    /// Completely zeroed, allows for retrieving the current kernel values
    pub fn retrieve() -> Self {
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

    // Helper function to create a dummy Timex for `NoopClockAdjuster`.
    #[cfg(feature = "test-side-by-side")]
    pub fn create_dummy_timex() -> Timex {
        Timex(timex {
            modes: 0,
            offset: 0,
            freq: 0,
            maxerror: 0,
            esterror: 0,
            status: 0,
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
}

impl AsRef<timex> for Timex {
    fn as_ref(&self) -> &timex {
        &self.0
    }
}

#[cfg(test)]
mod test {
    use libc::{STA_NANO, timeval};
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

    // Helper function to create a Timex with custom time and status values
    fn create_timex_with_time(tv_sec: i64, tv_usec: i64, status: i32) -> Timex {
        Timex(timex {
            modes: 0,
            offset: 0,
            freq: 0,
            maxerror: 0,
            esterror: 0,
            status,
            constant: 0,
            precision: 0,
            tolerance: 0,
            #[cfg(not(target_os = "macos"))]
            time: timeval { tv_sec, tv_usec },
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

    #[test]
    fn test_get_time_microsecond_precision_without_sta_nano() {
        // Test that without STA_NANO flag, tv_usec is interpreted as microseconds
        let timex = create_timex_with_time(1, 123_456, 0);
        let result = timex.time();
        let expected = Instant::from_secs(1) + Duration::from_micros(123_456);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_get_time_nanosecond_precision_with_sta_nano() {
        // Test that with STA_NANO flag, tv_usec is interpreted as nanoseconds
        let timex = create_timex_with_time(1, 123_456_789, STA_NANO);
        let result = timex.time();
        let expected = Instant::from_secs(1) + Duration::from_nanos(123_456_789);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_get_time_zero_values() {
        // Test epoch time (zero values)
        let timex = create_timex_with_time(0, 0, 0);
        let result = timex.time();
        assert_eq!(result, Instant::UNIX_EPOCH);
    }

    #[test]
    fn test_get_time_negative_seconds() {
        // Test negative seconds (before epoch)
        let timex = create_timex_with_time(-10, 500_000, 0);
        let result = timex.time();
        let expected = Instant::from_secs(-10) + Duration::from_micros(500_000);
        assert_eq!(result, expected);
    }
}
