//! Value rendering for `cbctl status`

use clock_bound::daemon::time::tsc::{Frequency, Period, Skew};
use clock_bound::daemon::time::{Duration, Instant};

/// The timestamp format: the log files' format (`LOG_TIMESTAMP_FORMAT` in the daemon's logging
/// module) with nanoseconds instead of microseconds.
const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.9fZ";

/// Render a timestamp in RFC 3339, UTC, to the nanosecond: `2026-09-29T17:42:00.694821000Z`.
pub fn timestamp(time: Instant) -> String {
    chrono::DateTime::from_timestamp_nanos(time.as_nanos())
        .format(TIMESTAMP_FORMAT)
        .to_string()
}

/// Render how long ago an event was, given its age: `2s ago`.
pub fn ago(age: Duration) -> String {
    format!("{} ago", duration(age))
}

/// Render how far a deadline is from now, given `deadline - now`: `16m 38s left` while ahead,
/// `22s ago` once passed.
pub fn left_or_ago(remaining: Duration) -> String {
    let rendered = duration(remaining);
    if remaining >= Duration::ZERO {
        format!("{rendered} left")
    } else {
        format!("{rendered} ago")
    }
}

/// Render a duration in whole seconds, as the largest non-zero unit plus the next
/// one down, with zeros kept: `2s`, `16m 0s`, `3h 45m`, `1d 1h`.
///
/// The duration is rounded to the nearest second before the units are chosen, so 59.6 seconds is
/// `1m 0s`, not `60s`.
fn duration(duration: Duration) -> String {
    const NANOS_PER_SECOND: u64 = 1_000_000_000;
    const SECONDS_PER_MINUTE: u64 = 60;
    const MINUTES_PER_HOUR: u64 = 60;
    const HOURS_PER_DAY: u64 = 24;
    const SECONDS_PER_HOUR: u64 = SECONDS_PER_MINUTE * MINUTES_PER_HOUR;
    const SECONDS_PER_DAY: u64 = SECONDS_PER_HOUR * HOURS_PER_DAY;

    let nanos = duration.as_nanos().unsigned_abs();
    let seconds = nanos.saturating_add(NANOS_PER_SECOND / 2) / NANOS_PER_SECOND;
    let (minutes, hours, days) = (
        seconds / SECONDS_PER_MINUTE,
        seconds / SECONDS_PER_HOUR,
        seconds / SECONDS_PER_DAY,
    );
    if seconds < SECONDS_PER_MINUTE {
        format!("{seconds}s")
    } else if seconds < SECONDS_PER_HOUR {
        format!("{minutes}m {}s", seconds % SECONDS_PER_MINUTE)
    } else if seconds < SECONDS_PER_DAY {
        format!("{hours}h {}m", minutes % MINUTES_PER_HOUR)
    } else {
        format!("{days}d {}h", hours % HOURS_PER_DAY)
    }
}

/// Render an error bound in adaptive units with 4 significant digits rounded up, and whole
/// nanoseconds below a microsecond: `412 ns`, `67.60 us` for 67591 ns, `4.568 ms`.
#[expect(
    clippy::cast_precision_loss,
    reason = "the value is rounded to 4 significant digits first, which an f64 holds exactly"
)]
pub fn error_bound(bound: Duration) -> String {
    let nanos = bound.as_nanos().unsigned_abs();
    if nanos < 1_000 {
        return format!("{nanos} ns");
    }
    let nanos = ceil_to_significant_digits(nanos, 4) as f64;
    with_adaptive_unit(nanos, &ERROR_BOUND_UNITS, 4)
}

/// The smallest value at or above `value` with at most the given significant digits,
/// in integer arithmetic so that nothing is lost to floating point: `67591` to `67600` for 4
/// digits, not `67590`, which is below it. A carry adds a digit: `999960` to `1000000`.
fn ceil_to_significant_digits(value: u64, significant_digits: u32) -> u64 {
    let len = value.checked_ilog10().map_or(1, |log| log + 1);
    if len <= significant_digits {
        return value;
    }
    let step = 10_u64.pow(len - significant_digits);
    value.div_ceil(step) * step
}

/// Render a counter period, with adaptive units from nanoseconds up and 8 significant digits:
/// `0.29411765 ns`.
pub fn period(period: Period) -> String {
    with_adaptive_unit(period.get(), &PERIOD_UNITS, 8)
}

/// Render a counter frequency, with adaptive units and 8 significant digits: `3.4000000 GHz`.
pub fn frequency(frequency: Frequency) -> String {
    with_adaptive_unit(frequency.get(), &FREQUENCY_UNITS, 8)
}

/// Render a rate as a whole number of parts per billion, rounded up: `40 ppb`, `15040 ppb`,
/// `42 ppb` for 41.016 ppb.
pub fn rate(rate: Skew) -> String {
    let ppb = rate.get() * 1e9;
    // Floating-point noise in a rate decoded from the segment, ignored when rounding up, so a rate
    // a hair above a whole number of ppb, such as 61 ppb decoded as 61.00000000000001, stays
    // `61 ppb`.
    let noise = 1e-6;
    // Adding 0.0 turns the -0.0 that `ceil` returns just below zero into 0.0, which prints as `0`.
    format!("{:.0} ppb", (ppb - noise).ceil() + 0.0)
}

/// A unit of an adaptive notation: its name, and how many of the notation's base unit make one.
struct Unit {
    name: &'static str,
    in_base_units: f64,
}

/// Error bound units, smallest first. The base unit is the nanosecond; below a microsecond,
/// bounds print as whole nanoseconds.
const ERROR_BOUND_UNITS: [Unit; 3] = [
    Unit {
        name: "us",
        in_base_units: 1e3,
    },
    Unit {
        name: "ms",
        in_base_units: 1e6,
    },
    Unit {
        name: "s",
        in_base_units: 1e9,
    },
];

/// Counter period units, smallest first. The base unit is the second.
const PERIOD_UNITS: [Unit; 4] = [
    Unit {
        name: "ns",
        in_base_units: 1e-9,
    },
    Unit {
        name: "us",
        in_base_units: 1e-6,
    },
    Unit {
        name: "ms",
        in_base_units: 1e-3,
    },
    Unit {
        name: "s",
        in_base_units: 1.0,
    },
];

/// Counter frequency units, smallest first. The base unit is the hertz.
const FREQUENCY_UNITS: [Unit; 4] = [
    Unit {
        name: "Hz",
        in_base_units: 1.0,
    },
    Unit {
        name: "kHz",
        in_base_units: 1e3,
    },
    Unit {
        name: "MHz",
        in_base_units: 1e6,
    },
    Unit {
        name: "GHz",
        in_base_units: 1e9,
    },
];

/// Render `value`, given in the base unit of `units`, in the largest unit in which the number
/// shown is at least 1, with the given `significant_digits`:
///
/// - 67600 ns, with the error bound units and 4 digits: `67.60 us`, not `0.06760 ms`.
/// - 3.4e9 Hz, with the frequency units and 8 digits: `3.4000000 GHz`.
/// - 2.9411765e-10 s, with the period units and 8 digits: `0.29411765 ns`. The value is below one
///   nanosecond, the smallest unit, so it is shown in nanoseconds.
///
/// When rounding carries the number to 1000, the next unit up is used: 999.999996 MHz at 8
/// digits is `1.0000000 GHz`, not `1000.0000 MHz`.
fn with_adaptive_unit(value: f64, units: &[Unit], significant_digits: usize) -> String {
    // The largest unit in which the number is at least 1, or the smallest unit if there is none:
    // `us` for 67600 ns, `ms` for 4568000 ns, and `ns` for a 0.294 ns counter period.
    let best_fit_index = units
        .iter()
        .rposition(|unit| value.abs() >= unit.in_base_units)
        .unwrap_or(0);
    // That unit and the larger ones, which rounding can carry into: `[us, ms, s]` from `us`.
    let candidate_units = &units[best_fit_index..];
    // The largest candidate, used if rounding carries in every smaller one, and the smaller ones
    // in order: `s`, then `[us, ms]`.
    let (largest_unit, smaller_units) = candidate_units
        .split_last()
        .expect("a notation has at least one unit");
    for unit in smaller_units {
        let rendered_number =
            with_significant_digits(value / unit.in_base_units, significant_digits);
        // Rounding can carry the number to 1000, as with 999.999996 MHz at 8 digits, which
        // renders as `1000.0000`: try the next unit up, `1.0000000 GHz`.
        let rounded_to_1000 = rendered_number
            .parse::<f64>()
            .is_ok_and(|parsed| parsed.abs() >= 1000.0);
        if !rounded_to_1000 {
            return format!("{rendered_number} {}", unit.name);
        }
    }
    let rendered_number =
        with_significant_digits(value / largest_unit.in_base_units, significant_digits);
    format!("{rendered_number} {}", largest_unit.name)
}

/// Render `value` with `significant_digits` significant digits, keeping trailing zeros:
/// `3.4000000` for 8 digits. Integer digits beyond `significant_digits` are kept rather than
/// rounded away.
fn with_significant_digits(value: f64, significant_digits: usize) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    // `{:.N}` sets the digits after the point, so find the exponent of the leading digit first.
    // Scientific notation gives it after rounding: 9.996 at 3 digits is `1.00e1`, exponent 1.
    let precision = significant_digits.saturating_sub(1);
    let scientific = format!("{value:.precision$e}");
    let exponent = scientific
        .split_once('e')
        .and_then(|(_, exponent)| exponent.parse::<isize>().ok())
        .unwrap_or(0);
    // 67.6 at 4 digits: 3 - 1 = 2 decimals, `67.60`.
    // 0.294 at 3: 2 + 1 = 3, `0.294`.
    // 1234 at 3: 2 - 3, clamped to 0, `1234`.
    let decimals = precision.saturating_sub_signed(exponent);
    format!("{value:.decimals$}")
}

#[cfg(test)]
mod test {
    use rstest::rstest;

    use super::*;

    #[test]
    fn timestamp_keeps_every_nanosecond() {
        assert_eq!(
            timestamp(Instant::from_time(1_790_703_720, 694_821_000)),
            "2026-09-29T17:42:00.694821000Z"
        );
        assert_eq!(
            timestamp(Instant::from_time(1_790_703_720, 694_821_999)),
            "2026-09-29T17:42:00.694821999Z"
        );
        assert_eq!(timestamp(Instant::EPOCH), "1970-01-01T00:00:00.000000000Z");
    }

    /// Assert the duration rule: the largest non-zero unit plus the next one down, zeros kept,
    /// whole seconds rounded before the units are chosen.
    #[rstest]
    #[case(Duration::ZERO, "0s")]
    #[case(Duration::from_millis(2_400), "2s")]
    #[case(Duration::from_millis(9_960), "10s")]
    #[case(Duration::from_millis(59_400), "59s")]
    #[case(Duration::from_millis(59_600), "1m 0s")]
    #[case(Duration::from_secs(784), "13m 4s")]
    #[case(Duration::from_secs(960), "16m 0s")]
    #[case(Duration::from_millis(997_600), "16m 38s")]
    #[case(Duration::from_millis(3_599_600), "1h 0m")]
    #[case(Duration::from_secs(3_600), "1h 0m")]
    #[case(Duration::from_secs(13_542), "3h 45m")]
    #[case(Duration::from_secs(86_399), "23h 59m")]
    #[case(Duration::from_secs(90_000), "1d 1h")]
    fn duration_uses_the_largest_unit_and_the_next_one_down(
        #[case] length: Duration,
        #[case] expected: &str,
    ) {
        assert_eq!(duration(length), expected);
    }

    /// Assert that a past event renders with `ago`, and a deadline with `left` until it passes.
    #[test]
    fn relative_times_put_the_direction_after_the_number() {
        assert_eq!(ago(Duration::from_millis(2_400)), "2s ago");
        assert_eq!(ago(Duration::from_nanos(-1)), "0s ago");
        assert_eq!(left_or_ago(Duration::from_millis(997_600)), "16m 38s left");
        assert_eq!(left_or_ago(Duration::ZERO), "0s left");
        assert_eq!(left_or_ago(Duration::from_secs(-22)), "22s ago");
    }

    /// Assert error bounds in adaptive units with 4 significant digits rounded up, including the
    /// carries from 999.96 us to milliseconds and from 9.999001 ms to 10 ms.
    #[rstest]
    #[case(0, "0 ns")]
    #[case(412, "412 ns")]
    #[case(999, "999 ns")]
    #[case(1_000, "1.000 us")]
    #[case(1_001, "1.001 us")]
    #[case(4_567, "4.567 us")]
    #[case(31_500, "31.50 us")]
    #[case(67_591, "67.60 us")]
    #[case(67_596, "67.60 us")]
    #[case(67_600, "67.60 us")]
    #[case(999_960, "1.000 ms")]
    #[case(4_567_001, "4.568 ms")]
    #[case(4_567_784, "4.568 ms")]
    #[case(9_999_001, "10.00 ms")]
    #[case(11_822_860, "11.83 ms")]
    #[case(1_200_000_000, "1.200 s")]
    #[case(15_600_000_000, "15.60 s")]
    #[case(123_456_789_012, "123.5 s")]
    fn error_bound_uses_adaptive_units_and_rounds_up(#[case] nanos: i64, #[case] expected: &str) {
        assert_eq!(error_bound(Duration::from_nanos(nanos)), expected);
    }

    /// Assert counter periods and frequencies with 8 significant digits across counter rates,
    /// including a carry into the next unit.
    #[test]
    fn period_and_frequency_use_8_significant_digits() {
        assert_eq!(
            period(Period::from_seconds(999.999_999_96e-9)),
            "1.0000000 us"
        );
        assert_eq!(
            frequency(Frequency::from_hz(999_999_996.0)),
            "1.0000000 GHz"
        );
        assert_eq!(period(Period::from_seconds(1.0 / 3.4e9)), "0.29411765 ns");
        assert_eq!(frequency(Frequency::from_hz(3.4e9)), "3.4000000 GHz");
        assert_eq!(period(Period::from_seconds(1e-9)), "1.0000000 ns");
        assert_eq!(frequency(Frequency::from_hz(1e9)), "1.0000000 GHz");
        assert_eq!(period(Period::from_seconds(1.0 / 25e6)), "40.000000 ns");
        assert_eq!(frequency(Frequency::from_hz(25e6)), "25.000000 MHz");
        assert_eq!(
            frequency(Frequency::from_hz(1.0 / (1.0 / 3.4e9))),
            "3.4000000 GHz"
        );
        assert_eq!(
            period(Period::from_seconds(1.0 / 3.400_000_012e9)),
            "0.29411765 ns"
        );
        assert_eq!(
            frequency(Frequency::from_hz(3.400_000_012e9)),
            "3.4000000 GHz"
        );
    }

    /// Assert rates as whole ppb, rounded up whatever their size, with floating-point noise just
    /// above a whole number ignored.
    #[test]
    fn rate_is_whole_ppb_rounded_up() {
        assert_eq!(rate(Skew::from_ppb(40.0)), "40 ppb");
        assert_eq!(rate(Skew::from_ppb(15_000.0 + 40e-9 * 1e9)), "15040 ppb");
        assert_eq!(rate(Skew::from_ppb(58.594)), "59 ppb");
        assert_eq!(rate(Skew::from_ppb(41.016)), "42 ppb");
        assert_eq!(rate(Skew::from_ppb(41.000_01)), "42 ppb");
        assert_eq!(rate(Skew::from_ppb(0.4)), "1 ppb");
        assert_eq!(rate(Skew::from_ppb(0.0)), "0 ppb");
        assert_eq!(rate(Skew::from_ppb(1e-9)), "0 ppb");
        assert_eq!(rate(Skew::from_ppb(61e-9 * 1e9)), "61 ppb");
        assert_eq!(rate(Skew::from_ppb(30_000.000_000_000_004)), "30000 ppb");
        assert_eq!(rate(Skew::from_ppb(1_234_600.0)), "1234600 ppb");
    }

    #[test]
    fn ceil_to_significant_digits_keeps_short_values_and_carries() {
        assert_eq!(ceil_to_significant_digits(0, 4), 0);
        assert_eq!(ceil_to_significant_digits(999, 4), 999);
        assert_eq!(ceil_to_significant_digits(9_999, 4), 9_999);
        assert_eq!(ceil_to_significant_digits(10_000, 4), 10_000);
        assert_eq!(ceil_to_significant_digits(10_001, 4), 10_010);
        assert_eq!(ceil_to_significant_digits(99_991, 4), 100_000);
        assert_eq!(
            ceil_to_significant_digits(u64::MAX / 2, 4),
            9_224_000_000_000_000_000
        );
    }

    #[test]
    fn with_significant_digits_handles_carry_and_trailing_zeros() {
        assert_eq!(with_significant_digits(9.996, 3), "10.0");
        assert_eq!(with_significant_digits(9.99, 3), "9.99");
        assert_eq!(with_significant_digits(0.000_999_96, 3), "0.00100");
        assert_eq!(with_significant_digits(1234.0, 3), "1234");
        assert_eq!(with_significant_digits(0.0, 4), "0.000");
        assert_eq!(with_significant_digits(-9.996, 3), "-10.0");
        assert_eq!(with_significant_digits(f64::INFINITY, 4), "inf");
        assert_eq!(with_significant_digits(f64::NAN, 4), "NaN");
        assert_eq!(with_significant_digits(0.294, 0), "0.3");
    }
}
