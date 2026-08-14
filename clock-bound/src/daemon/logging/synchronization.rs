//! Synchronization log entry types and emit helpers.
//!
//! The synchronization log (`synchronization.log`) is the primary log for investigating
//! time synchronization behavior. It provides a concise record of the selected clock's
//! state and system clock discipline activity.
//!
//! ## Usage
//!
//! ```ignore
//! use crate::daemon::logging::synchronization::log_clock_status_change;
//!
//! log_clock_status_change(ClockStatus::Unknown, ClockStatus::Synchronized);
//! ```

use serde::Serialize;

use super::{ClockStatusName, FFClock, Source};
use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_sync_algorithm::SourceInfo;
use crate::shm::ClockStatus;

/// Tracing target for the synchronization log.
pub const TARGET: &str = "clock_bound::synchronization";

/// Filename for the synchronization log.
pub const FILE_NAME: &str = "synchronization.log";

/// Log that the daemon has started to the synchronization log.
pub fn log_daemon_started() {
    let envelope = serde_json::json!({
        "event": "daemon_started",
    });
    emit(&envelope);
}

/// Log a clock status transition to the synchronization log.
pub fn log_clock_status_change(from: ClockStatus, to: ClockStatus) {
    let envelope = serde_json::json!({
        "event": "clock_status_change",
        "clock_status_change": {
            "from": ClockStatusName::from(from),
            "to": ClockStatusName::from(to),
        }
    });
    emit(&envelope);
}

/// Log a synchronization snapshot to the synchronization log.
pub fn log_sync_snapshot(
    source_info: Option<SourceInfo>,
    clock_parameters: Option<&ClockParameters>,
    clock_status: ClockStatus,
) {
    let source = source_info.map(Source::from);
    let ffclock = clock_parameters.map(|params| FFClock::from_params(params, clock_status));

    let envelope = serde_json::json!({
        "event": "synchronization",
        "synchronization": {
            "source": source,
            "ffclock": ffclock,
        }
    });
    emit(&envelope);
}

// This function requires a `StructuredLogLayer` to be registered for the `synchronization.log` target.
// If there is no such tracing layer registered, then this function will silently drop the logs.
fn emit(entry: &impl Serialize) {
    let json = serde_json::to_string(entry).unwrap();
    tracing::info!(target: TARGET, entry = json.as_str());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::time::{Duration, Instant, TscCount, tsc::Period};

    fn sample_clock_parameters() -> ClockParameters {
        ClockParameters {
            tsc_count: TscCount::new(1_000_000),
            time: Instant::from_secs(1_721_139_245), // 2024-07-16T14:14:05Z
            clock_error_bound: Duration::from_nanos(108_000),
            period: Period::from_seconds(1e-9),
            period_max_error: Period::from_seconds(1e-11),
            as_of_monotonic: Instant::new(0),
        }
    }

    // =========================================================================
    // End-to-end tests: wire up StructuredLogLayer + subscriber, call the public
    // log_* functions, and validate that the on-disk JSON matches embedded schema
    // examples.
    //
    // Each test includes a human-readable JSON example showing exactly what a
    // consumer of the log file will see. The `assert_matches_expected` helper
    // validates actual output against the example, checking:
    //   - Exact key set and ordering
    //   - Exact values for all fields except dynamic ones (timestamp)
    //   - Format validation for dynamic fields
    // =========================================================================

    mod end_to_end {
        use super::*;
        use crate::daemon::event::Stratum;
        use crate::daemon::logging::layer::StructuredLogLayer;
        use serde_json::{Map, Value};
        use std::fs::File;
        use std::io::Read;
        use std::net::SocketAddr;
        use tempfile::TempDir;
        use tracing_subscriber::layer::SubscriberExt;

        const TEST_VERSION: &str = "0.1.0-test";

        /// Set up a `StructuredLogLayer` targeting the synchronization log, run a
        /// closure with the subscriber active, and return the raw file contents.
        /// The `TempDir` is returned to keep it alive for the caller's scope.
        fn with_synchronization_layer(f: impl FnOnce()) -> (String, TempDir) {
            let tmp_dir = TempDir::new().unwrap();
            let (layer, _handle) =
                StructuredLogLayer::new(tmp_dir.path(), FILE_NAME, TARGET, TEST_VERSION);

            let subscriber = tracing_subscriber::registry().with(layer);
            tracing::subscriber::with_default(subscriber, f);

            let mut contents = String::new();
            File::open(tmp_dir.path().join(FILE_NAME))
                .unwrap()
                .read_to_string(&mut contents)
                .unwrap();
            (contents, tmp_dir)
        }

        /// Parse an output line into a Value, compare against expected JSON (copying
        /// the dynamic timestamp from actual into expected before assertion).
        fn assert_matches_expected(actual_line: &str, expected_json: &str) {
            let actual: Value =
                serde_json::from_str(actual_line).expect("actual output is not valid JSON");
            let mut expected: Value =
                serde_json::from_str(expected_json).expect("expected JSON is not valid JSON");

            // Timestamp is dynamic — copy from actual so comparison passes.
            expected["timestamp"] = actual["timestamp"].clone();

            assert_eq!(actual, expected);
        }

        #[test]
        fn log_daemon_started_schema() {
            let (output, _tmp) = with_synchronization_layer(log_daemon_started);

            assert_matches_expected(
                output.lines().next().unwrap(),
                r#"{
                    "timestamp": "",
                    "event": "daemon_started",
                    "version": "0.1.0-test"
                }"#,
            );
        }

        #[test]
        fn log_clock_status_change_schema() {
            let (output, _tmp) = with_synchronization_layer(|| {
                log_clock_status_change(ClockStatus::Unknown, ClockStatus::Synchronized);
            });

            assert_matches_expected(
                output.lines().next().unwrap(),
                r#"{
                    "timestamp": "",
                    "event": "clock_status_change",
                    "clock_status_change": {
                        "from": "UNKNOWN",
                        "to": "SYNCHRONIZED"
                    },
                    "version": "0.1.0-test"
                }"#,
            );
        }

        #[test]
        fn log_clock_status_change_all_variants() {
            let cases = [
                (
                    ClockStatus::Unknown,
                    ClockStatus::Synchronized,
                    "UNKNOWN",
                    "SYNCHRONIZED",
                ),
                (
                    ClockStatus::Synchronized,
                    ClockStatus::FreeRunning,
                    "SYNCHRONIZED",
                    "FREE_RUNNING",
                ),
                (
                    ClockStatus::FreeRunning,
                    ClockStatus::Disrupted,
                    "FREE_RUNNING",
                    "DISRUPTED",
                ),
                (
                    ClockStatus::Disrupted,
                    ClockStatus::Unknown,
                    "DISRUPTED",
                    "UNKNOWN",
                ),
            ];

            let (output, _tmp) = with_synchronization_layer(|| {
                for (from, to, _, _) in &cases {
                    log_clock_status_change(*from, *to);
                }
            });
            let lines: Vec<&str> = output.lines().collect();

            assert_eq!(lines.len(), cases.len());
            for (line, (_, _, expected_from, expected_to)) in lines.iter().zip(&cases) {
                let entry: Value = serde_json::from_str(line).unwrap();
                assert_eq!(entry["clock_status_change"]["from"], *expected_from);
                assert_eq!(entry["clock_status_change"]["to"], *expected_to);
            }
        }

        #[test]
        fn log_sync_snapshot_with_ntp_source_schema() {
            let params = sample_clock_parameters();
            let addr: SocketAddr = "169.254.169.123:123".parse().unwrap();

            let (output, _tmp) = with_synchronization_layer(|| {
                log_sync_snapshot(
                    Some(SourceInfo::AmazonTimeSync(addr, Stratum::ONE)),
                    Some(&params),
                    ClockStatus::Synchronized,
                );
            });

            assert_matches_expected(
                output.lines().next().unwrap(),
                r#"{
                    "timestamp": "",
                    "event": "synchronization",
                    "synchronization": {
                        "source": {
                            "type": "ntp",
                            "identifier": "169.254.169.123:123"
                        },
                        "ffclock": {
                            "time_sync": "2024-07-16T14:14:05.000000Z",
                            "clock_error_bound_ns": 108000,
                            "status": "SYNCHRONIZED"
                        }
                    },
                    "version": "0.1.0-test"
                }"#,
            );
        }

        #[test]
        fn log_sync_snapshot_with_phc_source_schema() {
            let params = sample_clock_parameters();

            let (output, _tmp) = with_synchronization_layer(|| {
                log_sync_snapshot(
                    Some(SourceInfo::Phc),
                    Some(&params),
                    ClockStatus::Synchronized,
                );
            });

            assert_matches_expected(
                output.lines().next().unwrap(),
                r#"{
                    "timestamp": "",
                    "event": "synchronization",
                    "synchronization": {
                        "source": {
                            "type": "phc",
                            "identifier": "/dev/ptp0"
                        },
                        "ffclock": {
                            "time_sync": "2024-07-16T14:14:05.000000Z",
                            "clock_error_bound_ns": 108000,
                            "status": "SYNCHRONIZED"
                        }
                    },
                    "version": "0.1.0-test"
                }"#,
            );
        }

        #[test]
        fn log_sync_snapshot_without_source_schema() {
            let (output, _tmp) = with_synchronization_layer(|| {
                log_sync_snapshot(None, None, ClockStatus::Unknown);
            });

            assert_matches_expected(
                output.lines().next().unwrap(),
                r#"{
                    "timestamp": "",
                    "event": "synchronization",
                    "synchronization": {
                        "source": null,
                        "ffclock": null
                    },
                    "version": "0.1.0-test"
                }"#,
            );
        }

        #[test]
        fn output_is_newline_delimited_json() {
            let (output, _tmp) = with_synchronization_layer(|| {
                log_daemon_started();
                log_clock_status_change(ClockStatus::Unknown, ClockStatus::Synchronized);
                log_sync_snapshot(None, None, ClockStatus::Unknown);
            });

            let lines: Vec<&str> = output.lines().collect();
            assert_eq!(lines.len(), 3);
            for line in &lines {
                let parsed: Result<Map<String, Value>, _> = serde_json::from_str(line);
                assert!(parsed.is_ok(), "line is not valid JSON: {line}");
            }
            assert!(output.ends_with('\n'), "file should end with a newline");
        }
    }

    #[test]
    fn emit_is_noop_without_tracing_layer() {
        use tempfile::TempDir;

        let tmp_dir = TempDir::new().unwrap();
        let log_path = tmp_dir.path().join(FILE_NAME);

        // No subscriber is set up — emit should silently succeed without panic
        // and nothing should be written to disk.
        let entry = serde_json::json!({"event": "daemon_started"});
        emit(&entry);

        assert!(
            !log_path.exists(),
            "log file should not be created when no tracing layer is registered"
        );
    }
}
