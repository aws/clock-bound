//! Ordering signals, anomaly classification and run summaries.
//!
//! Log lines carry two independent physical ordering signals:
//!
//! - the **inner timestamp**, assigned at emission by the daemon's tracing layer.
//!   Monotone in emission order, except across legitimate clock steps (the wall
//!   clock is the clock under adjustment).
//! - the **counter** (`tsc_post` / `counter_post`), captured at I/O completion.
//!   Monotone in completion order, which can legitimately differ from the order
//!   events were fed to the algorithm when source I/O windows overlap (e.g. a
//!   fast PHC read completing inside a long NTP exchange).
//!
//! Comparing how each signal evolves against file order classifies an anomaly:
//!
//! | timestamp regresses | counter regresses | classification                         |
//! |---------------------|-------------------|----------------------------------------|
//! | yes                 | yes               | delivery scramble (e.g. `CloudWatch`)  |
//! | yes                 | no                | clock step: real daemon behavior       |
//! | no                  | yes               | async I/O reordering: real behavior    |
//!
//! Only a delivery scramble means the file order is wrong: both physical signals
//! agree the line was displaced in transit. The other two are the daemon's own
//! dynamics and must be preserved to replay what the algorithm experienced.

use std::fmt;

/// Ordering signals extracted from a raw log line.
///
/// Extraction is format-agnostic: it works on both the current ffevents format
/// (`"counter_pre"` / `"counter_post"`, plain JSON) and the legacy primer format
/// (`"tsc_pre"` / `"tsc_post"`, nested inside an escaped JSON string).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OrderingSignals {
    /// Inner timestamp (RFC3339 with fixed precision: lexicographic order is
    /// chronological order)
    pub timestamp: Option<String>,
    /// Counter reading at I/O completion, when the record becomes available to
    /// the daemon
    pub counter_post: Option<u64>,
}

impl OrderingSignals {
    /// Extract ordering signals from a raw log line.
    pub fn extract(line: &str) -> Self {
        Self {
            timestamp: extract_string_value(line, "\"timestamp\":"),
            counter_post: extract_counter(line, "_post"),
        }
    }
}

/// Extract the string value following `key` (e.g. `"timestamp":"..."`).
fn extract_string_value(line: &str, key: &str) -> Option<String> {
    let start = line.find(key)? + key.len();
    let rest = line[start..].trim_start_matches('"');
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Extract a counter value for the given suffix (`_pre` or `_post`), accepting
/// both the `counter` (ffevents) and legacy `tsc` (primer) key names, in plain or
/// escaped-JSON-string form.
fn extract_counter(line: &str, suffix: &str) -> Option<u64> {
    for name in ["counter", "tsc"] {
        let key = format!("{name}{suffix}");
        if let Some(pos) = line.find(&key) {
            // Skip the escaping/quoting between the key and the number: `":` or `\":`
            let digits: String = line[pos + key.len()..]
                .chars()
                .skip_while(|&c| matches!(c, '\\' | '"' | ':' | ' '))
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(value) = digits.parse() {
                return Some(value);
            }
        }
    }
    None
}

/// Classification of an out-of-order observation. See the module documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderAnomalyKind {
    /// Both timestamp and counter regressed: the line was displaced in transit
    /// (e.g. `CloudWatch` delivery). File order is wrong.
    DeliveryScramble,
    /// Timestamp regressed but the counter did not: the daemon stepped its
    /// clock. File order is correct.
    ClockStep,
    /// Counter regressed but the timestamp did not: I/O completion order
    /// differed from processing order. File order is correct.
    AsyncReorder,
}

impl fmt::Display for OrderAnomalyKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeliveryScramble => write!(f, "delivery scramble"),
            Self::ClockStep => write!(f, "clock step"),
            Self::AsyncReorder => write!(f, "async I/O reordering"),
        }
    }
}

/// An out-of-order observation, with magnitudes ("by how much").
///
/// Invariant: at least one of the two regression fields is `Some` (an anomaly is
/// only constructed when something regressed); the combination determines the
/// [`kind`](Self::kind).
#[derive(Debug, Clone, PartialEq)]
pub struct OrderAnomaly {
    /// 0-based index of the offending line
    pub line_index: usize,
    /// How far the timestamp went backwards, in nanoseconds, if it regressed
    pub timestamp_regression_ns: Option<i64>,
    /// How far the counter went backwards, in ticks, if it regressed
    pub counter_regression_ticks: Option<u64>,
}

impl OrderAnomaly {
    /// Classification of this anomaly, derived from which signals regressed.
    /// See the module documentation.
    pub fn kind(&self) -> OrderAnomalyKind {
        match (
            &self.timestamp_regression_ns,
            &self.counter_regression_ticks,
        ) {
            (Some(_), Some(_)) => OrderAnomalyKind::DeliveryScramble,
            (Some(_), None) => OrderAnomalyKind::ClockStep,
            (None, Some(_)) => OrderAnomalyKind::AsyncReorder,
            (None, None) => unreachable!("anomaly constructed with at least one regression"),
        }
    }
}

impl fmt::Display for OrderAnomaly {
    #[expect(
        clippy::cast_precision_loss,
        reason = "display only: regressions of 2^52 ns (~52 days) are not meaningful"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line_index, self.kind())?;
        if let Some(ns) = self.timestamp_regression_ns {
            write!(f, ", timestamp back {:.3} ms", ns as f64 / 1e6)?;
        }
        if let Some(ticks) = self.counter_regression_ticks {
            write!(f, ", counter back {ticks} ticks")?;
        }
        Ok(())
    }
}

/// Tracks ordering signals across successive lines and classifies anomalies.
///
/// Timestamps and counters are tracked independently, so lines missing one signal
/// (e.g. init/disruption records carry no counter) do not disturb the other.
#[derive(Debug, Default)]
pub struct OrderTracker {
    last_timestamp: Option<String>,
    last_counter_post: Option<u64>,
    summary: OrderSummary,
}

impl OrderTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe the signals of the next line, in file order.
    ///
    /// Returns an anomaly classification if the line is out of order with
    /// respect to previously observed lines.
    pub fn observe(
        &mut self,
        line_index: usize,
        signals: &OrderingSignals,
    ) -> Option<OrderAnomaly> {
        self.summary.lines_observed += 1;

        let ts_regression = match (&signals.timestamp, &self.last_timestamp) {
            (Some(cur), Some(last)) if cur < last => {
                Some(timestamp_delta_ns(last, cur).unwrap_or(0))
            }
            _ => None,
        };
        let counter_regression = match (signals.counter_post, self.last_counter_post) {
            (Some(cur), Some(last)) if cur < last => Some(last - cur),
            _ => None,
        };

        if let Some(ts) = &signals.timestamp {
            self.last_timestamp = Some(ts.clone());
        }
        if let Some(counter) = signals.counter_post {
            self.last_counter_post = Some(counter);
        }

        if ts_regression.is_none() && counter_regression.is_none() {
            return None;
        }

        let anomaly = OrderAnomaly {
            line_index,
            timestamp_regression_ns: ts_regression,
            counter_regression_ticks: counter_regression,
        };
        self.summary.record(&anomaly);
        Some(anomaly)
    }

    /// End-of-run summary of everything observed.
    pub fn summary(&self) -> &OrderSummary {
        &self.summary
    }
}

/// End-of-run summary of ordering observations.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct OrderSummary {
    pub lines_observed: usize,
    pub delivery_scrambles: usize,
    pub clock_steps: usize,
    pub async_reorders: usize,
    /// Largest timestamp regression observed, in nanoseconds
    pub max_timestamp_regression_ns: i64,
    /// Largest counter regression observed, in ticks
    pub max_counter_regression_ticks: u64,
}

impl OrderSummary {
    fn record(&mut self, anomaly: &OrderAnomaly) {
        match anomaly.kind() {
            OrderAnomalyKind::DeliveryScramble => self.delivery_scrambles += 1,
            OrderAnomalyKind::ClockStep => self.clock_steps += 1,
            OrderAnomalyKind::AsyncReorder => self.async_reorders += 1,
        }
        if let Some(ns) = anomaly.timestamp_regression_ns {
            self.max_timestamp_regression_ns = self.max_timestamp_regression_ns.max(ns);
        }
        if let Some(ticks) = anomaly.counter_regression_ticks {
            self.max_counter_regression_ticks = self.max_counter_regression_ticks.max(ticks);
        }
    }
}

impl fmt::Display for OrderSummary {
    #[expect(
        clippy::cast_precision_loss,
        reason = "display only: regressions of 2^52 ns (~52 days) are not meaningful"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} lines observed: {} delivery scramble(s), {} clock step(s), {} async I/O reordering(s)",
            self.lines_observed, self.delivery_scrambles, self.clock_steps, self.async_reorders
        )?;
        if self.max_timestamp_regression_ns > 0 {
            write!(
                f,
                ", max timestamp regression {:.3} ms",
                self.max_timestamp_regression_ns as f64 / 1e6
            )?;
        }
        if self.max_counter_regression_ticks > 0 {
            write!(
                f,
                ", max counter regression {} ticks",
                self.max_counter_regression_ticks
            )?;
        }
        Ok(())
    }
}

/// Nanoseconds between two RFC3339 timestamps (`later` minus `earlier`).
fn timestamp_delta_ns(later: &str, earlier: &str) -> Option<i64> {
    let later = chrono::DateTime::parse_from_rfc3339(later).ok()?;
    let earlier = chrono::DateTime::parse_from_rfc3339(earlier).ok()?;
    (later - earlier).num_nanoseconds()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real lines (truncated payloads) from the two formats
    const FFEVENTS_LINE: &str = r#"{"timestamp":"2026-07-16T14:33:49.654418Z","version":"3.0.0-alpha.2","event":"feed","feed":{"input":{"type":"ntp","identifier":"169.254.169.123:123","counter_pre":482393264419,"counter_post":482393405301,"data":{}},"output":null}}"#;
    const PRIMER_LINE: &str = r#"{"timestamp":"2025-12-29T07:59:58.825610Z","level":"INFO","fields":{"message":"feed","event":"{\"Phc\":{\"tsc_pre\":302084682318463,\"tsc_post\":302084682325383,\"data\":{}}}"},"target":"clock_bound::primer"}"#;
    const NO_COUNTER_LINE: &str = r#"{"timestamp":"2025-12-28T23:42:54.399311Z","level":"INFO","fields":{"message":"Clock Sync Alg started","init":"CSA initialized"}}"#;

    #[test]
    fn extracts_signals_from_ffevents_format() {
        let signals = OrderingSignals::extract(FFEVENTS_LINE);
        assert_eq!(
            signals.timestamp.as_deref(),
            Some("2026-07-16T14:33:49.654418Z")
        );
        assert_eq!(signals.counter_post, Some(482393405301));
    }

    #[test]
    fn extracts_signals_from_primer_format() {
        let signals = OrderingSignals::extract(PRIMER_LINE);
        assert_eq!(
            signals.timestamp.as_deref(),
            Some("2025-12-29T07:59:58.825610Z")
        );
        assert_eq!(signals.counter_post, Some(302084682325383));
    }

    #[test]
    fn extracts_partial_signals_from_counterless_line() {
        let signals = OrderingSignals::extract(NO_COUNTER_LINE);
        assert!(signals.timestamp.is_some());
        assert_eq!(signals.counter_post, None);
    }

    fn signals(ts: &str, post: Option<u64>) -> OrderingSignals {
        OrderingSignals {
            timestamp: Some(format!("2026-01-01T00:00:{ts}Z")),
            counter_post: post,
        }
    }

    #[test]
    fn classifies_delivery_scramble() {
        let mut tracker = OrderTracker::new();
        assert_eq!(tracker.observe(0, &signals("02.000000", Some(2000))), None);
        let anomaly = tracker
            .observe(1, &signals("01.000000", Some(1000)))
            .unwrap();
        assert_eq!(anomaly.kind(), OrderAnomalyKind::DeliveryScramble);
        assert_eq!(anomaly.timestamp_regression_ns, Some(1_000_000_000));
        assert_eq!(anomaly.counter_regression_ticks, Some(1000));
    }

    #[test]
    fn classifies_clock_step() {
        let mut tracker = OrderTracker::new();
        tracker.observe(0, &signals("02.000000", Some(1000)));
        let anomaly = tracker
            .observe(1, &signals("01.000000", Some(2000)))
            .unwrap();
        assert_eq!(anomaly.kind(), OrderAnomalyKind::ClockStep);
        assert_eq!(anomaly.timestamp_regression_ns, Some(1_000_000_000));
        assert_eq!(anomaly.counter_regression_ticks, None);
    }

    #[test]
    fn classifies_async_reorder() {
        let mut tracker = OrderTracker::new();
        tracker.observe(0, &signals("01.000000", Some(2000)));
        let anomaly = tracker
            .observe(1, &signals("02.000000", Some(1500)))
            .unwrap();
        assert_eq!(anomaly.kind(), OrderAnomalyKind::AsyncReorder);
        assert_eq!(anomaly.timestamp_regression_ns, None);
        assert_eq!(anomaly.counter_regression_ticks, Some(500));
    }

    #[test]
    fn counterless_lines_do_not_disturb_counter_tracking() {
        let mut tracker = OrderTracker::new();
        tracker.observe(0, &signals("01.000000", Some(1000)));
        // A counter-less line in between (e.g. init) with an in-order timestamp
        let init = OrderingSignals {
            timestamp: Some("2026-01-01T00:00:01.500000Z".to_string()),
            ..Default::default()
        };
        assert_eq!(tracker.observe(1, &init), None);
        // Counter comparison still keys on the last counter-carrying line
        assert_eq!(tracker.observe(2, &signals("02.000000", Some(1100))), None);
        assert_eq!(tracker.summary().lines_observed, 3);
    }

    #[test]
    fn summary_accumulates() {
        let mut tracker = OrderTracker::new();
        tracker.observe(0, &signals("05.000000", Some(5000)));
        tracker.observe(1, &signals("04.000000", Some(4000))); // scramble
        tracker.observe(2, &signals("03.000000", Some(6000))); // step
        tracker.observe(3, &signals("06.000000", Some(5500))); // async
        let summary = tracker.summary();
        assert_eq!(summary.lines_observed, 4);
        assert_eq!(summary.delivery_scrambles, 1);
        assert_eq!(summary.clock_steps, 1);
        assert_eq!(summary.async_reorders, 1);
        assert_eq!(summary.max_timestamp_regression_ns, 1_000_000_000);
        assert_eq!(summary.max_counter_regression_ticks, 1000);
    }
}
