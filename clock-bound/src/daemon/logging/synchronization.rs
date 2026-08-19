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

use super::{ClockStatusName, FFClock, LogEvent, Source, StructuredLog, emit_event};
use crate::daemon::clock_sync_algorithm::SyncParameters;
use crate::shm::ClockStatus;

#[cfg(feature = "daemon")]
use crate::daemon::clock_state::adjust_data::ClockAdjustData;

/// Body of a `daemon_started` event, emitted once when the daemon starts.
#[derive(Serialize)]
struct DaemonStarted {
    message: &'static str,
}

impl LogEvent for DaemonStarted {
    const STREAM: StructuredLog = StructuredLog::Synchronization;
    const EVENT: &'static str = "daemon_started";
}

/// Body of a `clock_status_change` event, capturing a clock status transition.
#[derive(Serialize)]
struct ClockStatusChange {
    from: ClockStatusName,
    to: ClockStatusName,
}

impl LogEvent for ClockStatusChange {
    const STREAM: StructuredLog = StructuredLog::Synchronization;
    const EVENT: &'static str = "clock_status_change";
}

/// Body of a `synchronization` snapshot event.
#[derive(Serialize)]
struct SyncSnapshot {
    source: Option<Source>,
    ffclock: Option<FFClock>,
}

impl LogEvent for SyncSnapshot {
    const STREAM: StructuredLog = StructuredLog::Synchronization;
    const EVENT: &'static str = "synchronization";
}

/// Body of a `system_clock_step` event.
#[cfg(feature = "daemon")]
#[derive(Serialize)]
struct SystemClockStep {
    /// Signed phase correction applied to `CLOCK_REALTIME`, in nanoseconds.
    /// Negative means the clock was moved backward.
    step_ns: i64,
}

#[cfg(feature = "daemon")]
impl LogEvent for SystemClockStep {
    const STREAM: StructuredLog = StructuredLog::Synchronization;
    const EVENT: &'static str = "system_clock_step";
}

#[cfg(feature = "daemon")]
/// Signed min/max/mean of the smooth phase corrections observed over a window,
/// in nanoseconds.
#[derive(Serialize)]
struct CorrectionNs {
    min: i64,
    max: i64,
    mean: i64,
}

#[cfg(feature = "daemon")]
impl From<&ClockAdjustData> for CorrectionNs {
    fn from(data: &ClockAdjustData) -> Self {
        Self {
            min: data.min_ns(),
            max: data.max_ns(),
            mean: data.mean_ns(),
        }
    }
}

/// Body of a `system_clock_summary` event, emitted periodically to summarize the
/// smooth phase corrections fed to the kernel PLL over the preceding window.
#[cfg(feature = "daemon")]
#[derive(Serialize)]
struct SystemClockSummary {
    window_width_s: u64,
    correction_count: u64,
    /// Signed min/max/mean of the corrections, or `null` when there were none.
    correction_ns: Option<CorrectionNs>,
}

#[cfg(feature = "daemon")]
impl LogEvent for SystemClockSummary {
    const STREAM: StructuredLog = StructuredLog::Synchronization;
    const EVENT: &'static str = "system_clock_summary";
}

/// Log that the daemon has started to the synchronization log.
pub fn log_daemon_started() {
    emit_event(&DaemonStarted {
        message: "ClockBound daemon started",
    });
}

/// Log a clock status transition to the synchronization log.
pub fn log_clock_status_change(from: ClockStatus, to: ClockStatus) {
    emit_event(&ClockStatusChange {
        from: from.into(),
        to: to.into(),
    });
}

/// Log a synchronization snapshot to the synchronization log.
pub fn log_sync_snapshot(sync_parameters: Option<&SyncParameters>, clock_status: ClockStatus) {
    let source = sync_parameters.map(Source::from);
    let ffclock =
        sync_parameters.map(|sp| FFClock::from_params(&sp.clock_parameters, clock_status));

    emit_event(&SyncSnapshot { source, ffclock });
}

/// Log a one-time system clock step to the synchronization log.
#[cfg(feature = "daemon")]
pub fn log_system_clock_step(step_ns: i64) {
    emit_event(&SystemClockStep { step_ns });
}

/// Log a windowed summary of smooth phase corrections to the synchronization log.
///
/// `correction_ns` is `null` when the window contained no corrections
/// (`adjust_data.count() == 0`).
#[cfg(feature = "daemon")]
pub(crate) fn log_system_clock_summary(window_width_s: u64, adjust_data: &ClockAdjustData) {
    let correction_ns = if adjust_data.count() > 0 {
        Some(CorrectionNs::from(adjust_data))
    } else {
        None
    };
    emit_event(&SystemClockSummary {
        window_width_s,
        correction_count: adjust_data.count(),
        correction_ns,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::clock_parameters::ClockParameters;
    use crate::daemon::clock_sync_algorithm::SourceInfo;
    use crate::daemon::time::{Duration, Instant, TscCount, tsc::Period};
    use indoc::indoc;

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
        use crate::daemon::logging::{
            StructuredLog,
            test_support::{assert_matches_expected, with_layer},
        };
        use serde_json::{Map, Value};
        use std::net::SocketAddr;
        use tempfile::TempDir;

        const TEST_VERSION: &str = "0.1.0-test";

        /// Set up a `StructuredLogLayer` targeting the synchronization log, run a
        /// closure with the subscriber active, and return the raw file contents.
        /// The `TempDir` is returned to keep it alive for the caller's scope.
        fn with_synchronization_layer(f: impl FnOnce()) -> (String, TempDir) {
            with_layer(StructuredLog::Synchronization, TEST_VERSION, f)
        }

        #[test]
        fn log_daemon_started_schema() {
            let (output, _tmp) = with_synchronization_layer(log_daemon_started);

            assert_matches_expected(
                output.lines().next().unwrap(),
                indoc! {r#"
                {
                    "timestamp": "",
                    "event": "daemon_started",
                    "daemon_started": {
                        "message": "ClockBound daemon started"
                    },
                    "version": "0.1.0-test"
                }
                "#},
            );
        }

        #[test]
        fn log_clock_status_change_schema() {
            let (output, _tmp) = with_synchronization_layer(|| {
                log_clock_status_change(ClockStatus::Unknown, ClockStatus::Synchronized);
            });

            assert_matches_expected(
                output.lines().next().unwrap(),
                indoc! {r#"
                {
                    "timestamp": "",
                    "event": "clock_status_change",
                    "clock_status_change": {
                        "from": "UNKNOWN",
                        "to": "SYNCHRONIZED"
                    },
                    "version": "0.1.0-test"
                }
                "#},
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
            let addr: SocketAddr = "169.254.169.123:123".parse().unwrap();
            let sync_parameters = SyncParameters {
                clock_parameters: sample_clock_parameters(),
                source_info: SourceInfo::AmazonTimeSync(addr, Stratum::ONE),
                selected_at: Instant::from_secs(1_721_139_180),
                selected_at_clock_error_bound: Duration::from_nanos(95_000),
            };

            let (output, _tmp) = with_synchronization_layer(|| {
                log_sync_snapshot(Some(&sync_parameters), ClockStatus::Synchronized);
            });

            assert_matches_expected(
                output.lines().next().unwrap(),
                indoc! {r#"
                {
                    "timestamp": "",
                    "event": "synchronization",
                    "synchronization": {
                        "source": {
                            "type": "ntp",
                            "identifier": "169.254.169.123:123",
                            "selected_at": "2024-07-16T14:13:00.000000Z",
                            "clock_error_bound_ns": 95000
                        },
                        "ffclock": {
                            "time_sync": "2024-07-16T14:14:05.000000Z",
                            "clock_error_bound_ns": 108000,
                            "status": "SYNCHRONIZED"
                        }
                    },
                    "version": "0.1.0-test"
                }
                "#},
            );
        }

        #[test]
        fn log_sync_snapshot_with_phc_source_schema() {
            let sync_parameters = SyncParameters {
                clock_parameters: sample_clock_parameters(),
                source_info: SourceInfo::Phc("/dev/ptp0".into()),
                selected_at: Instant::from_secs(1_721_139_180),
                selected_at_clock_error_bound: Duration::from_nanos(95_000),
            };

            let (output, _tmp) = with_synchronization_layer(|| {
                log_sync_snapshot(Some(&sync_parameters), ClockStatus::Synchronized);
            });

            assert_matches_expected(
                output.lines().next().unwrap(),
                indoc! {r#"
                {
                    "timestamp": "",
                    "event": "synchronization",
                    "synchronization": {
                        "source": {
                            "type": "phc",
                            "identifier": "/dev/ptp0",
                            "selected_at": "2024-07-16T14:13:00.000000Z",
                            "clock_error_bound_ns": 95000
                        },
                        "ffclock": {
                            "time_sync": "2024-07-16T14:14:05.000000Z",
                            "clock_error_bound_ns": 108000,
                            "status": "SYNCHRONIZED"
                        }
                    },
                    "version": "0.1.0-test"
                }
                "#},
            );
        }

        #[test]
        fn log_sync_snapshot_without_source_schema() {
            let (output, _tmp) = with_synchronization_layer(|| {
                log_sync_snapshot(None, ClockStatus::Unknown);
            });

            assert_matches_expected(
                output.lines().next().unwrap(),
                indoc! {r#"
                {
                    "timestamp": "",
                    "event": "synchronization",
                    "synchronization": {
                        "source": null,
                        "ffclock": null
                    },
                    "version": "0.1.0-test"
                }
                "#},
            );
        }

        #[cfg(feature = "daemon")]
        #[test]
        fn log_system_clock_step_schema() {
            let (output, _tmp) = with_synchronization_layer(|| {
                log_system_clock_step(-842_000_000);
            });

            assert_matches_expected(
                output.lines().next().unwrap(),
                indoc! {r#"
                {
                    "timestamp": "",
                    "event": "system_clock_step",
                    "system_clock_step": {
                        "step_ns": -842000000
                    },
                    "version": "0.1.0-test"
                }
                "#},
            );
        }

        #[cfg(feature = "daemon")]
        #[test]
        fn log_system_clock_summary_schema() {
            let (output, _tmp) = with_synchronization_layer(|| {
                let mut adjust_data = ClockAdjustData::default();
                // min -4800, max -3200, sum -16400 over 4 → mean -4100.
                for correction in [-4800, -3200, -4200, -4200] {
                    adjust_data.record(correction);
                }
                log_system_clock_summary(600, &adjust_data);
            });

            assert_matches_expected(
                output.lines().next().unwrap(),
                indoc! {r#"
                {
                    "timestamp": "",
                    "event": "system_clock_summary",
                    "system_clock_summary": {
                        "window_width_s": 600,
                        "correction_count": 4,
                        "correction_ns": {
                            "min": -4800,
                            "max": -3200,
                            "mean": -4100
                        }
                    },
                    "version": "0.1.0-test"
                }
                "#},
            );
        }

        #[cfg(feature = "daemon")]
        #[test]
        fn log_system_clock_summary_empty_window_schema() {
            let (output, _tmp) = with_synchronization_layer(|| {
                log_system_clock_summary(600, &ClockAdjustData::default());
            });

            assert_matches_expected(
                output.lines().next().unwrap(),
                indoc! {r#"
                {
                    "timestamp": "",
                    "event": "system_clock_summary",
                    "system_clock_summary": {
                        "window_width_s": 600,
                        "correction_count": 0,
                        "correction_ns": null
                    },
                    "version": "0.1.0-test"
                }
                "#},
            );
        }

        #[test]
        fn output_is_newline_delimited_json() {
            let (output, _tmp) = with_synchronization_layer(|| {
                log_daemon_started();
                log_clock_status_change(ClockStatus::Unknown, ClockStatus::Synchronized);
                log_sync_snapshot(None, ClockStatus::Unknown);
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
        let log_path = tmp_dir
            .path()
            .join(StructuredLog::Synchronization.file_name());

        // No subscriber is set up — emit should silently succeed without panic
        // and nothing should be written to disk.
        log_daemon_started();

        assert!(
            !log_path.exists(),
            "log file should not be created when no tracing layer is registered"
        );
    }
}
