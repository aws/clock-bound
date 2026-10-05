//! Synchronization algorithm analysis and tracing
//!
//! This module emits internal clock synchronization algorithm calculations for data analysis. It
//! exists solely for `algorithm` feature consumers (e.g. ff-tester).
//!
//! `ffevents.log` records what the algorithm was given and what it published. This module's records
//! go to `algo_analysis.log` and describe the internals of each layer of the calculation — one
//! record *kind* per layer, emitted by a tracepoint (`algo_trace!`, in `algo_trace.rs`). This is
//! instrumentation at the point where the layer knows the value.
//!
//! The stream exists for replays only: in a daemon build this module is not compiled and the
//! tracepoints expand to a use of their arguments and nothing else.
//!
//! Records share the standard structured-log envelope
//! `{"event": "<kind>", "seq", "<kind>": { ...body... }, "version"}`.
//! `event` names the kind. `seq` is the feed event's sequence number: the algorithm opens a `feed`
//! span carrying it around each event and the logging layer stamps it onto every record emitted
//! inside. It is the key that joins a record to its feed row and kinds to each other.

use crate::daemon::{
    logging::{LogEvent, StructuredLog},
    time::{Duration, tsc::Period},
};

/// The theta calculation internals: the offset estimate, the bound over, and characterization of
/// the observation window and more.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct TraceTheta {
    /// The correction applied to the uncorrected clock.
    pub theta_ns: Duration,
    /// The corresponding calculated clock error bound
    pub clock_error_bound_ns: Duration,
    /// The algorithm's own share of the clock error bound: the largest sample error on the
    /// observation window.
    pub delay_bound_ns: Duration,
    /// Samples of the theta window that took part in the weighted average.
    pub samples_used: usize,
    /// Samples of the theta window the RTT filter rejected.
    pub samples_rejected: usize,
    /// The current sample's naive offset against the uncorrected clock.
    pub offset_ns: Duration,
    /// The current sample's weight in the weighted average, in (0, 1]; 1 for the window's best RTT.
    pub weight: f64,
    /// The sum of the weights of the samples used.
    pub weight_sum: f64,
}

impl LogEvent for TraceTheta {
    const STREAM: StructuredLog = StructuredLog::AlgoAnalysis;
    const EVENT: &'static str = "theta";
}

/// The local period estimate over the period window.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct TraceLocalPeriod {
    /// The oscillator local period estimate.
    pub period_local: Period,
    /// Its error (seconds per tick).
    pub period_local_error: Period,
    /// Samples of the period window the estimate was made from.
    pub samples: usize,
    /// The duration between the two samples the estimate is built from.
    pub baseline_ns: Duration,
    /// The age of the most recent sample the estimate is built from, relative to the sample being
    /// processed.
    pub age_ns: Duration,
}

impl LogEvent for TraceLocalPeriod {
    const STREAM: StructuredLog = StructuredLog::AlgoAnalysis;
    const EVENT: &'static str = "local_period";
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value};

    use super::*;
    use crate::daemon::{
        algo_trace,
        logging::{StructuredLog, test_support::with_layer},
    };

    const TEST_VERSION: &str = "test-version";

    fn emit_theta() {
        algo_trace!(TraceTheta {
            theta_ns: Duration::from(-1_348),
            clock_error_bound_ns: Duration::from(27_528),
            delay_bound_ns: Duration::from(4_112),
            samples_used: 98,
            samples_rejected: 2,
            offset_ns: Duration::from(-350),
            weight: 0.8,
            weight_sum: 61.5,
        });
    }

    fn parse_single_record(output: &str) -> Map<String, Value> {
        let mut lines = output.lines();
        let record = serde_json::from_str(lines.next().expect("expected one record"))
            .expect("expected valid JSON");
        assert!(lines.next().is_none(), "expected exactly one record");
        record
    }

    /// Envelope and body keys, in on-disk order, with `seq` taken from the enclosing `feed` span.
    #[test]
    fn theta_record_inside_a_feed_span() {
        let (output, _tmp) = with_layer(StructuredLog::AlgoAnalysis, TEST_VERSION, || {
            let _feed = tracing::trace_span!(
                target: StructuredLog::AlgoAnalysis.target(),
                "feed",
                seq = 42_u64
            )
            .entered();
            emit_theta();
        });
        let record = parse_single_record(&output);
        let keys: Vec<&str> = record.keys().map(String::as_str).collect();
        assert_eq!(keys, ["timestamp", "event", "seq", "theta", "version"]);
        assert_eq!(record["event"], "theta");
        assert_eq!(record["seq"], Value::from(42_u64));
        let body = record["theta"]
            .as_object()
            .expect("theta body is a JSON object");
        let body_keys: Vec<&str> = body.keys().map(String::as_str).collect();
        assert_eq!(
            body_keys,
            [
                "theta_ns",
                "clock_error_bound_ns",
                "delay_bound_ns",
                "samples_used",
                "samples_rejected",
                "offset_ns",
                "weight",
                "weight_sum",
            ]
        );
        // Durations are exact JSON integers (nanoseconds); the weights are plain numbers.
        assert_eq!(body["theta_ns"], Value::from(-1_348_i64));
        assert_eq!(body["delay_bound_ns"], Value::from(4_112_i64));
        assert_eq!(body["offset_ns"], Value::from(-350_i64));
        assert_eq!(body["weight"], Value::from(0.8));
        assert_eq!(body["weight_sum"], Value::from(61.5));
    }

    /// Outside any span the record has no `seq`: the layer stamps what the span says, nothing else.
    #[test]
    fn record_outside_a_span_has_no_seq() {
        let (output, _tmp) = with_layer(StructuredLog::AlgoAnalysis, TEST_VERSION, emit_theta);
        let record = parse_single_record(&output);
        assert!(!record.contains_key("seq"));
    }

    #[test]
    fn local_period_record() {
        let (output, _tmp) = with_layer(StructuredLog::AlgoAnalysis, TEST_VERSION, || {
            algo_trace!(TraceLocalPeriod {
                period_local: Period::from_seconds(4.166e-10),
                period_local_error: Period::from_seconds(1e-16),
                samples: 512,
                baseline_ns: Duration::from_secs(45),
                age_ns: Duration::from_secs(3),
            });
        });
        let record = parse_single_record(&output);
        assert_eq!(record["event"], "local_period");
        assert_eq!(record["local_period"]["samples"], Value::from(512_u64));
        assert_eq!(
            record["local_period"]["baseline_ns"],
            Value::from(45_000_000_000_i64)
        );
        assert_eq!(
            record["local_period"]["age_ns"],
            Value::from(3_000_000_000_i64)
        );
    }
}
