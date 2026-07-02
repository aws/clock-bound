//! String parse functionality for time types

// developers note. Functionality is needed for setting values
// in `ff-tester` for high level parameters. Do not need to set
// sub-nanosecond level granularity
pub trait DurationParse: Sized {
    fn from_nanoseconds(nanos: i64) -> Self;

    /// Parse from a duration of time
    ///
    /// Takes in a signed integer plus a suffix of
    /// - days
    /// - hours
    /// - minutes
    /// - seconds
    /// - milliseconds
    /// - microseconds
    /// - nanoseconds
    ///
    /// Without a suffix, nanoseconds are the default type
    ///
    /// # Errors
    /// Errors if
    fn parse_from_duration(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (rest, duration) =
            nom::character::complete::digit1::<_, ()>(s).map_err(|_| "Failed to parse duration")?;
        let duration = duration.parse::<i64>().map_err(|_| "i64 parsing failed")?;

        let suffix = rest.trim();

        match suffix.to_lowercase().as_str() {
            "" => Ok(Self::from_nanoseconds(duration)),
            "ns" | "nano" | "nanos" | "nanosecond" | "nanoseconds" => {
                Ok(Self::from_nanoseconds(duration))
            }
            "us" | "micro" | "micros" | "microsecond" | "microseconds" => {
                Ok(Self::from_nanoseconds(duration * NANOS_PER_MICRO))
            }
            "ms" | "milli" | "millis" | "millisecond" | "milliseconds" => {
                Ok(Self::from_nanoseconds(duration * NANOS_PER_MILLI))
            }
            "s" | "sec" | "secs" | "second" | "seconds" => {
                Ok(Self::from_nanoseconds(duration * NANOS_PER_SEC))
            }
            "m" | "min" | "mins" | "minute" | "minutes" => Ok(Self::from_nanoseconds(
                duration * NANOS_PER_SEC * SECS_PER_MINUTE,
            )),
            "h" | "hr" | "hrs" | "hour" | "hours" => Ok(Self::from_nanoseconds(
                duration * NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR,
            )),
            "d" | "day" | "days" => Ok(Self::from_nanoseconds(
                duration * NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR * HOURS_PER_DAY,
            )),
            _ => Err(format!("Unknown suffix: {suffix}")),
        }
    }
}

impl<T: super::NanoType + Copy> DurationParse for super::Time<T> {
    fn from_nanoseconds(nanos: i64) -> Self {
        Self::from_nanos(nanos)
    }
}

impl<T: super::NanoType + Copy> DurationParse for super::Diff<T> {
    fn from_nanoseconds(nanos: i64) -> Self {
        Self::from_nanos(nanos)
    }
}

impl<T: super::NanoType + Copy> std::str::FromStr for super::Time<T> {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_from_duration(s)
    }
}

impl<T: super::NanoType + Copy> std::str::FromStr for super::Diff<T> {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_from_duration(s)
    }
}

const NANOS_PER_SEC: i64 = 1_000_000_000;
const NANOS_PER_MILLI: i64 = 1_000_000;
const NANOS_PER_MICRO: i64 = 1_000;
const SECS_PER_MINUTE: i64 = 60;
const MINS_PER_HOUR: i64 = 60;
const HOURS_PER_DAY: i64 = 24;

#[cfg(test)]
mod test {
    use super::*;
    use crate::daemon::time::{Duration, Instant};

    use rstest::rstest;

    #[rstest]
    #[case("1", 1)]
    #[case("1ns", 1)]
    #[case("1nano", 1)]
    #[case("1nanos", 1)]
    #[case("1nanosecond", 1)]
    #[case("1nanoseconds", 1)]
    #[case("1us", NANOS_PER_MICRO)]
    #[case("1micro", NANOS_PER_MICRO)]
    #[case("1micros", NANOS_PER_MICRO)]
    #[case("1microsecond", NANOS_PER_MICRO)]
    #[case("1microseconds", NANOS_PER_MICRO)]
    #[case("1ms", NANOS_PER_MILLI)]
    #[case("1milli", NANOS_PER_MILLI)]
    #[case("1millis", NANOS_PER_MILLI)]
    #[case("1millisecond", NANOS_PER_MILLI)]
    #[case("1milliseconds", NANOS_PER_MILLI)]
    #[case("1s", NANOS_PER_SEC)]
    #[case("1sec", NANOS_PER_SEC)]
    #[case("1secs", NANOS_PER_SEC)]
    #[case("1second", NANOS_PER_SEC)]
    #[case("1seconds", NANOS_PER_SEC)]
    #[case("1m", NANOS_PER_SEC * SECS_PER_MINUTE)]
    #[case("1min", NANOS_PER_SEC * SECS_PER_MINUTE)]
    #[case("1mins", NANOS_PER_SEC * SECS_PER_MINUTE)]
    #[case("1minute", NANOS_PER_SEC * SECS_PER_MINUTE)]
    #[case("1minutes", NANOS_PER_SEC * SECS_PER_MINUTE)]
    #[case("1h", NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR)]
    #[case("1hr", NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR)]
    #[case("1hrs", NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR)]
    #[case("1hour", NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR)]
    #[case("1hours", NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR)]
    #[case("1d", NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR * HOURS_PER_DAY)]
    #[case("1day", NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR * HOURS_PER_DAY)]
    #[case("1days", NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR * HOURS_PER_DAY)]
    fn test_duration_parse(#[case] input: &str, #[case] expected: i64) {
        let duration = Duration::parse_from_duration(input).unwrap();
        assert_eq!(duration.as_nanos(), expected);
        let instant = Instant::parse_from_duration(input).unwrap();
        assert_eq!(instant.as_nanos(), expected);
    }

    #[rstest]
    #[case::trimming("  1s  ", NANOS_PER_SEC)]
    #[case::case_insensitivity("1S", NANOS_PER_SEC)]
    #[case::uppercase("1SEC", NANOS_PER_SEC)]
    fn test_duration_parse_edge_cases(#[case] input: &str, #[case] expected: i64) {
        let duration = Duration::parse_from_duration(input).unwrap();
        assert_eq!(duration.as_nanos(), expected);
    }

    #[rstest]
    #[case("")]
    #[case("abc")]
    #[case("1x")]
    #[case("seconds")]
    #[case("1.5s")]
    #[case("-1s")]
    fn test_duration_parse_errors(#[case] input: &str) {
        let _ = Duration::parse_from_duration(input).unwrap_err();
    }
}
