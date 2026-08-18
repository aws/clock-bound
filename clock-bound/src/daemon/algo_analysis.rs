//! Synchronization algorithm analysis tooling
//!
//! This module emits internal clock synchronization algorithm calculations for data analysis. It
//! exists solely for `algorithm` feature consumers (e.g. ff-tester).
//!
//! In daemon only mode (without the specific `algorithm` feature) everything here compiles away and
//! no extra logging is emitted.
//!
//! Records to emit are serialized with serde and tunneled through tracing via the shared
//! [`emit_event`](crate::daemon::logging::emit_event) helper, bound to the
//! [`StructuredLog::AlgoAnalysis`](crate::daemon::logging::StructuredLog::AlgoAnalysis) stream and
//! decorated by [`StructuredLogLayer`](crate::daemon::logging::layer::StructuredLogLayer).
//!
//! Records share the standard structured-log envelope
//! `{"timestamp", "event": "<name>", "<name>": { ...body... }, "version"}`. The `event`
//! discriminator lets multiple record types share the log file; each record body carries a
//! `source` field identifying the algorithm flavor.

use crate::daemon::{
    clock_sync_algorithm::ff::{LocalPeriodAndError, UncorrectedClock},
    logging::{LogEvent, StructuredLog, emit_event},
    time::{Duration, Instant, TscCount, tsc::Period},
};

/// Internal values of an uncorrected clock offset calculation (theta).
///
/// Serializes to the *body* of a `theta_calculation` event; [`emit_event`] wraps it in the
/// standard envelope.
///
/// FIXME: the keys for this record are subject to change and not completely defined yet
#[derive(Debug, Clone, serde::Serialize)]
struct ThetaCalculation {
    /// Time source
    source: &'static str,
    /// The correction applied to the uncorrected clock
    theta_ns: Duration,
    /// The clock error bound used in calculation
    clock_error_bound_ns: Duration,
    /// Estimated period of the uncorrected clock
    period: Period,
    /// Epoch of the uncorrected clock
    uncorrected_k_ns: Instant,
    /// Local period estimate (f64 seconds per tick)
    local_period: Period,
    /// Local period estimation error (f64 seconds per tick)
    local_period_error: Period,
    /// Counter reading the calculation was anchored to
    counter_midpoint: TscCount,
    /// Corrected time at counter midpoint
    time_ns: Instant,
}

impl LogEvent for ThetaCalculation {
    const STREAM: StructuredLog = StructuredLog::AlgoAnalysis;
    const EVENT: &'static str = "theta_calculation";
}

/// Emit the internal values of a theta calculation.
pub(crate) fn emit_theta_calculation(
    source: &'static str,
    theta: Duration,
    clock_error_bound: Duration,
    uncorrected_clock: UncorrectedClock,
    local_period: &LocalPeriodAndError,
    counter_midpoint: TscCount,
    time_ns: Instant,
) {
    let snapshot = ThetaCalculation {
        source,
        theta_ns: theta,
        clock_error_bound_ns: clock_error_bound,
        period: uncorrected_clock.p_estimate,
        uncorrected_k_ns: uncorrected_clock.k,
        local_period: local_period.period_local,
        local_period_error: local_period.error,
        counter_midpoint,
        time_ns,
    };

    emit_event(&snapshot);
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value};

    use super::*;
    use crate::daemon::logging::{StructuredLog, test_support::with_layer};

    const TEST_VERSION: &str = "test-version";

    /// Run `f` with a StructuredLogLayer capturing
    /// [`StructuredLog::AlgoAnalysis`] records to a temp file, and return the
    /// file contents. The [`tempfile::TempDir`] is returned to keep it alive for
    /// the caller's scope.
    fn with_algo_analysis_layer(f: impl FnOnce()) -> (String, tempfile::TempDir) {
        with_layer(StructuredLog::AlgoAnalysis, TEST_VERSION, f)
    }

    /// Emit one record built from easily recognizable values.
    fn emit_test_record() {
        emit_theta_calculation(
            "phc",
            Duration::from(-1_348), // theta: -1348 ns
            Duration::from(27_528), // clock error bound: 27528 ns
            UncorrectedClock {
                p_estimate: Period::from_seconds(9.523_683_333_064_784e-10),
                k: Instant::from(1_766_707_502_857_115_074),
            },
            &LocalPeriodAndError {
                period_local: Period::from_seconds(9.523_683_333_064_784e-10),
                error: Period::from_seconds(9.065_418_261_058_853e-14),
            },
            TscCount::from(270_765_653_528_298),
            Instant::from(1_766_965_370_641_132_697),
        );
    }

    fn parse_single_record(output: &str) -> Map<String, Value> {
        let mut lines = output.lines();
        let record = serde_json::from_str(lines.next().expect("expected one record"))
            .expect("expected valid JSON");
        assert!(lines.next().is_none(), "expected exactly one record");
        record
    }

    #[test]
    fn emits_one_record_routed_to_the_analysis_log() {
        let (output, _tmp) = with_algo_analysis_layer(emit_test_record);
        let record = parse_single_record(&output);
        assert_eq!(record["event"], "theta_calculation");
        assert_eq!(record["theta_calculation"]["source"], "phc");
    }

    /// The exact key set is the analysis schema: changing it breaks downstream
    /// analysis tooling, so a change here must be deliberate. Records use the
    /// standard nested envelope, so the top level carries the discriminator and
    /// the data fields live inside `theta_calculation`.
    #[test]
    fn record_schema_is_stable() {
        let (output, _tmp) = with_algo_analysis_layer(emit_test_record);
        let record = parse_single_record(&output);

        let keys: Vec<&str> = record.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                // injected by StructuredLogLayer
                "timestamp",
                // envelope discriminator + nested content key (both from LogEvent::EVENT)
                "event",
                "theta_calculation",
                // injected by StructuredLogLayer
                "version",
            ]
        );

        let body = record["theta_calculation"]
            .as_object()
            .expect("theta_calculation body is a JSON object");
        let body_keys: Vec<&str> = body.keys().map(String::as_str).collect();
        assert_eq!(
            body_keys,
            [
                "source",
                "theta_ns",
                "clock_error_bound_ns",
                "period",
                "uncorrected_k_ns",
                "local_period",
                "local_period_error",
                "counter_midpoint",
                "time_ns",
            ]
        );
    }

    /// Times, durations and counters are emitted as exact JSON integers
    /// (i64 nanoseconds / counts), including values beyond 2^53 that would not
    /// survive an f64 representation.
    #[test]
    fn integer_fields_are_exact_i64() {
        let (output, _tmp) = with_algo_analysis_layer(emit_test_record);
        let record = parse_single_record(&output);
        let body = &record["theta_calculation"];
        assert_eq!(body["theta_ns"], Value::from(-1_348_i64));
        assert_eq!(body["clock_error_bound_ns"], Value::from(27_528_i64));
        assert_eq!(
            body["uncorrected_k_ns"],
            Value::from(1_766_707_502_857_115_074_i64)
        );
        assert_eq!(body["time_ns"], Value::from(1_766_965_370_641_132_697_i64));
        assert_eq!(
            body["counter_midpoint"],
            Value::from(270_765_653_528_298_i64)
        );
    }

    /// Periods are emitted as f64 and survive the serialize -> layer parse ->
    /// re-serialize round trip bit-identically (shortest round-trip formatting).
    #[test]
    fn period_fields_round_trip_bit_identically() {
        let (output, _tmp) = with_algo_analysis_layer(emit_test_record);
        let record = parse_single_record(&output);
        let body = &record["theta_calculation"];
        let period = body["period"].as_f64().unwrap();
        assert_eq!(period.to_bits(), 9.523_683_333_064_784e-10_f64.to_bits());
        let error = body["local_period_error"].as_f64().unwrap();
        assert_eq!(error.to_bits(), 9.065_418_261_058_853e-14_f64.to_bits());
    }

    /// Without a subscriber interested in the target, emission is a silent no-op.
    #[test]
    fn emission_without_subscriber_is_a_no_op() {
        emit_test_record();
    }
}
