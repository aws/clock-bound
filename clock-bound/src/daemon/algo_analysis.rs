//! Synchronization algorithm analysis tooling
//!
//! This module emits internal clock synchronization algorithm calculations for data analysis. It
//! exists solely for `algorithm` feature consumers (e.g. ff-tester).
//!
//! In daemon only mode (without the specific `algorithm` feature) everything here compiles away and
//! no extra logging is emitted.
//!
//! Records to emit are serialized with serde and tunneled through tracing as a single `entry`
//! string field under [`ALGO_ANALYSIS_TARGET`](crate::daemon::logging::ALGO_ANALYSIS_TARGET),
//! and using [`StructuredLogLayer`](crate::daemon::logging::layer::StructuredLogLayer), which
//! decorates them..
//!
//! Every record carries an `event` discriminator field so multiple record types can share the log
//! file, plus a `source` field identifying the algorithm flavor.

use crate::daemon::{
    clock_sync_algorithm::ff::{LocalPeriodAndError, UncorrectedClock},
    time::{Duration, Instant, TscCount, tsc::Period},
};

/// Internal values of an uncorrected clock offset calculation (theta)
/// FIXME: the keys for this record are subject to change and not completely defined yet
#[derive(Debug, Clone, serde::Serialize)]
struct ThetaCalculation {
    /// Event type
    event: &'static str,
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
    {
        let snapshot = ThetaCalculation {
            event: "theta_calculation",
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

        // The `entry` field must be recorded as a str: StructuredLogLayer's visitor
        // only implements `record_str` and silently drops anything else.
        if let Ok(json) = serde_json::to_string(&snapshot) {
            tracing::trace!(
                target: crate::daemon::logging::ALGO_ANALYSIS_TARGET,
                entry = json.as_str(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use serde_json::{Map, Value};
    use tempfile::TempDir;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use crate::daemon::logging::{ALGO_ANALYSIS_TARGET, layer::StructuredLogLayer};

    /// Run `f` with a StructuredLogLayer capturing [`ALGO_ANALYSIS_TARGET`] records
    /// to a temp file, and return the file contents.
    fn with_algo_analysis_layer(f: impl FnOnce()) -> String {
        let tmp_dir = TempDir::new().unwrap();
        let (layer, _handle) = StructuredLogLayer::new(
            tmp_dir.path(),
            "algo_analysis.log",
            ALGO_ANALYSIS_TARGET,
            "test-version",
        );
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, f);

        let mut contents = String::new();
        std::fs::File::open(tmp_dir.path().join("algo_analysis.log"))
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        contents
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
        let output = with_algo_analysis_layer(emit_test_record);
        let record = parse_single_record(&output);
        assert_eq!(record["event"], "theta_calculation");
        assert_eq!(record["source"], "phc");
    }

    /// The exact key set is the analysis schema: changing it breaks downstream
    /// analysis tooling, so a change here must be deliberate.
    #[test]
    fn record_schema_is_stable() {
        let output = with_algo_analysis_layer(emit_test_record);
        let record = parse_single_record(&output);
        let keys: Vec<&str> = record.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                // injected by StructuredLogLayer
                "timestamp",
                // ThetaCalculation fields
                "event",
                "source",
                "theta_ns",
                "clock_error_bound_ns",
                "period",
                "uncorrected_k_ns",
                "local_period",
                "local_period_error",
                "counter_midpoint",
                "time_ns",
                // injected by StructuredLogLayer
                "version",
            ]
        );
    }

    /// Times, durations and counters are emitted as exact JSON integers
    /// (i64 nanoseconds / counts), including values beyond 2^53 that would not
    /// survive an f64 representation.
    #[test]
    fn integer_fields_are_exact_i64() {
        let output = with_algo_analysis_layer(emit_test_record);
        let record = parse_single_record(&output);
        assert_eq!(record["theta_ns"], Value::from(-1_348_i64));
        assert_eq!(record["clock_error_bound_ns"], Value::from(27_528_i64));
        assert_eq!(
            record["uncorrected_k_ns"],
            Value::from(1_766_707_502_857_115_074_i64)
        );
        assert_eq!(
            record["time_ns"],
            Value::from(1_766_965_370_641_132_697_i64)
        );
        assert_eq!(
            record["counter_midpoint"],
            Value::from(270_765_653_528_298_i64)
        );
    }

    /// Periods are emitted as f64 and survive the serialize -> layer parse ->
    /// re-serialize round trip bit-identically (shortest round-trip formatting).
    #[test]
    fn period_fields_round_trip_bit_identically() {
        let output = with_algo_analysis_layer(emit_test_record);
        let record = parse_single_record(&output);
        let period = record["period"].as_f64().unwrap();
        assert_eq!(period.to_bits(), 9.523_683_333_064_784e-10_f64.to_bits());
        let error = record["local_period_error"].as_f64().unwrap();
        assert_eq!(error.to_bits(), 9.065_418_261_058_853e-14_f64.to_bits());
    }

    /// Without a subscriber interested in the target, emission is a silent no-op.
    #[test]
    fn emission_without_subscriber_is_a_no_op() {
        emit_test_record();
    }
}
