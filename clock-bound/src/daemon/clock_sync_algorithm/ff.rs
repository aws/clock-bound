//! Flavours of clock sync algorithms
//!
//! This is where math happens

pub mod event_buffer;

mod ntp;
pub use ntp::Ntp;

mod phc;
pub use phc::Phc;

mod uncorrected_clock;
pub use uncorrected_clock::UncorrectedClock;

use crate::daemon::event::{SourceCeb, TscRtt};
use crate::daemon::time::{Duration, TscDiff, tsc::Period};

/// Used as the output for `calculate_local_period_and_error` methods
#[derive(Debug, Clone, PartialEq)]
pub struct LocalPeriodAndError {
    /// The period estimate, in seconds per tick
    pub period_local: Period,
    /// Its error: an absolute error, in seconds per tick
    pub error: Period,
}

/// A min/median/max summary of a set of values (e.g. `Duration`s or `TscDiff`s).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatSummary<T> {
    pub min: T,
    pub median: T,
    pub max: T,
}

impl<T: Ord + Copy + Into<i64> + From<i64>> StatSummary<T> {
    /// Summarize an iterator of values, or `None` if it is empty.
    pub fn from(values: impl IntoIterator<Item = T>) -> Option<Self> {
        let mut values: Vec<T> = values.into_iter().collect();
        if values.is_empty() {
            return None;
        }
        values.sort_unstable();
        let len = values.len();
        let median = if len % 2 == 1 {
            values[len / 2]
        } else {
            let (lower, upper): (i64, i64) = (values[len / 2 - 1].into(), values[len / 2].into());
            T::from(lower.midpoint(upper))
        };
        Some(Self {
            min: values[0],
            median,
            max: values[len - 1],
        })
    }
}

impl StatSummary<TscDiff> {
    /// Scale a TSC-tick summary to a duration summary by applying `period` to each bound.
    fn scale(self, period: Period) -> StatSummary<Duration> {
        StatSummary {
            min: self.min * period,
            median: self.median * period,
            max: self.max * period,
        }
    }
}

/// Aggregate statistics over a source's sample buffer at snapshot time.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SkmSummary {
    /// The window the stats cover: the requested window, clamped to the buffer's own.
    pub window: Duration,
    /// Measurement latency (RTT) distribution, or `None` without samples or a period.
    pub latency: Option<StatSummary<Duration>>,
    /// Source-reported error bound distribution, or `None` without samples or a period.
    pub source_clock_error_bound: Option<StatSummary<Duration>>,
}

/// Summarize a source's sample buffer into an [`SkmSummary`] over the last `window`.
///
/// `window` is clamped to the buffer's own window, and the clamped value is reported as
/// [`SkmSummary::window`]. Only samples within that window contribute to the distributions.
///
/// `period` (the selected clock's) converts the TSC-denominated samples to durations and defines
/// the window cutoff; without it the distributions are `None` (the window is still reported).
fn skm_summary_of<T: TscRtt + SourceCeb + std::fmt::Debug>(
    buffer: &event_buffer::Local<T>,
    window: Duration,
    period: Option<Period>,
) -> SkmSummary {
    let window = window.min(buffer.window());

    let Some(period) = period else {
        return SkmSummary {
            window,
            latency: None,
            source_clock_error_bound: None,
        };
    };

    let start = buffer.window_start(window, period);
    let samples: Vec<&T> = buffer.iter().skip(start).collect();

    // Summarize latency in TSC ticks, then scale the min/median/max to a duration.
    let latency_ticks = StatSummary::from(samples.iter().map(|event| event.rtt()));
    let latency = latency_ticks.map(|ticks| ticks.scale(period));

    let source_clock_error_bound =
        StatSummary::from(samples.iter().map(|event| event.source_clock_error_bound()));

    SkmSummary {
        window,
        latency,
        source_clock_error_bound,
    }
}

/// Output from `calculate_theta` methods
struct CalculateThetaOutput {
    /// The time correction to be applied
    theta: Duration,
    /// The worst clock error bound used in calculation
    clock_error_bound: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::clock_sync_algorithm::ff::event_buffer::Local;
    use crate::daemon::event::{self, NtpData, Stratum};
    use crate::daemon::time::{Instant, TscCount, TscDiff};
    use std::num::NonZeroUsize;

    fn ns(n: i64) -> Duration {
        Duration::from_nanos(n)
    }

    #[test]
    fn from_empty_is_none() {
        assert_eq!(StatSummary::<Duration>::from([]), None);
    }

    #[test]
    fn from_single_value() {
        let summary = StatSummary::from([ns(42)]).unwrap();
        assert_eq!(summary.min, ns(42));
        assert_eq!(summary.median, ns(42));
        assert_eq!(summary.max, ns(42));
    }

    #[test]
    fn from_odd_count_sorts_and_picks_middle() {
        let summary = StatSummary::from([ns(30), ns(10), ns(20)]).unwrap();
        assert_eq!(summary.min, ns(10));
        assert_eq!(summary.median, ns(20));
        assert_eq!(summary.max, ns(30));
    }

    #[test]
    fn from_even_count_averages_the_two_middle_values() {
        // Sorted: 10, 20, 30, 40. The middles are 20 and 30.
        let summary = StatSummary::from([ns(40), ns(10), ns(30), ns(20)]).unwrap();
        assert_eq!(summary.min, ns(10));
        assert_eq!(summary.median, ns(25));
        assert_eq!(summary.max, ns(40));
    }

    #[test]
    fn from_two_values_is_their_midpoint() {
        let summary = StatSummary::from([ns(10), ns(30)]).unwrap();
        assert_eq!(summary.median, ns(20));
    }

    #[test]
    fn from_even_count_with_equal_middles_is_that_value() {
        let summary = StatSummary::from([ns(5), ns(20), ns(20), ns(90)]).unwrap();
        assert_eq!(summary.median, ns(20));
    }

    #[test]
    fn from_even_count_with_odd_sum_rounds_towards_zero() {
        assert_eq!(StatSummary::from([ns(10), ns(21)]).unwrap().median, ns(15));
        assert_eq!(
            StatSummary::from([ns(-21), ns(-10)]).unwrap().median,
            ns(-15)
        );
    }

    #[test]
    fn from_even_count_does_not_overflow() {
        let summary = StatSummary::from([ns(i64::MAX - 2), ns(i64::MAX)]).unwrap();
        assert_eq!(summary.median, ns(i64::MAX - 1));
    }

    #[test]
    fn from_summarizes_tsc_diffs() {
        let summary =
            StatSummary::from([TscDiff::new(300), TscDiff::new(100), TscDiff::new(200)]).unwrap();
        assert_eq!(summary.min, TscDiff::new(100));
        assert_eq!(summary.median, TscDiff::new(200));
        assert_eq!(summary.max, TscDiff::new(300));
    }

    #[test]
    fn from_even_count_of_tsc_diffs_averages_the_two_middle_values() {
        let summary = StatSummary::from([
            TscDiff::new(400),
            TscDiff::new(100),
            TscDiff::new(300),
            TscDiff::new(200),
        ])
        .unwrap();
        assert_eq!(summary.median, TscDiff::new(250));
    }

    /// An NTP event at 1 GHz: `counter_post = counter_pre + rtt`, with the given source CEB.
    fn ntp(counter_pre: i64, rtt: i64, root_dispersion: Duration) -> event::Ntp {
        event::Ntp::builder()
            .counter_pre(TscCount::new(counter_pre))
            .counter_post(TscCount::new(counter_pre + rtt))
            .ntp_data(NtpData {
                server_recv_time: Instant::from_secs(10),
                server_send_time: Instant::from_secs(10),
                root_delay: Duration::from_nanos(0),
                root_dispersion,
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap()
    }

    fn buffer_of(events: impl IntoIterator<Item = event::Ntp>) -> Local<event::Ntp> {
        let mut buffer = Local::new(NonZeroUsize::new(16).unwrap());
        for event in events {
            buffer.feed(event).unwrap();
        }
        buffer
    }

    /// 1 GHz: 1 tick = 1 ns, so a tick count scales to that many nanoseconds.
    fn period_1ghz() -> Period {
        Period::from_seconds(1e-9)
    }

    #[test]
    fn skm_summary_without_period_reports_window_but_no_distributions() {
        let buffer = buffer_of([ntp(1_000_000_000, 100, ns(30))]);
        let summary = skm_summary_of(&buffer, Duration::from_secs(600), None);
        // Window is still reported (clamped to the buffer's own); distributions need a period.
        assert_eq!(summary.window, Duration::from_secs(600));
        assert_eq!(summary.latency, None);
        assert_eq!(summary.source_clock_error_bound, None);
    }

    #[test]
    fn skm_summary_empty_buffer_has_window_but_no_distributions() {
        let buffer = buffer_of([]);
        let summary = skm_summary_of(&buffer, Duration::from_secs(600), Some(period_1ghz()));
        assert_eq!(summary.window, Duration::from_secs(600));
        assert_eq!(summary.latency, None);
        assert_eq!(summary.source_clock_error_bound, None);
    }

    #[test]
    fn skm_summary_summarizes_scaled_latency_and_source_error() {
        let buffer = buffer_of([
            ntp(1_000_000_000, 100, ns(30)),
            ntp(2_000_000_000, 300, ns(10)),
        ]);
        let summary = skm_summary_of(&buffer, Duration::from_secs(600), Some(period_1ghz()));

        // Latency is scaled to a duration: at 1 GHz, 100/300 ticks -> 100/300 ns.
        let latency = summary.latency.unwrap();
        assert_eq!(latency.min, ns(100));
        assert_eq!(latency.median, ns(200));
        assert_eq!(latency.max, ns(300));

        let ceb = summary.source_clock_error_bound.unwrap();
        assert_eq!(ceb.min, ns(10));
        assert_eq!(ceb.median, ns(20));
        assert_eq!(ceb.max, ns(30));
    }

    #[test]
    fn skm_summary_window_is_clamped_to_the_buffers_own() {
        let buffer = Local::<event::Ntp>::new(NonZeroUsize::new(16).unwrap())
            .with_window(Duration::from_secs(512));
        // A requested window larger than the buffer's own is clamped down to it.
        let summary = skm_summary_of(&buffer, Duration::from_secs(600), Some(period_1ghz()));
        assert_eq!(summary.window, Duration::from_secs(512));
    }

    #[test]
    fn skm_summary_excludes_samples_older_than_the_window() {
        // 1 GHz. The oldest sample (at 1s) is 5s before the newest (at 6s); a 2s window excludes
        // it, keeping only the samples at 5s and 6s.
        let buffer = buffer_of([
            ntp(1_000_000_000, 500, ns(99)),
            ntp(5_000_000_000, 100, ns(20)),
            ntp(6_000_000_000, 300, ns(10)),
        ]);
        let summary = skm_summary_of(&buffer, Duration::from_secs(2), Some(period_1ghz()));

        // The rtt-500/ceb-99 sample at 1s falls outside the 2s window.
        let latency = summary.latency.unwrap();
        assert_eq!(latency.min, ns(100));
        assert_eq!(latency.max, ns(300));
        let ceb = summary.source_clock_error_bound.unwrap();
        assert_eq!(ceb.min, ns(10));
        assert_eq!(ceb.max, ns(20));
        assert_eq!(summary.window, Duration::from_secs(2));
    }
}
