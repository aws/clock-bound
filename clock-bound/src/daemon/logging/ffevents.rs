//! Feed-forward events log (ffevents.log) envelope types and emission helpers.
//!
//! This module defines the typed structures for each event in the ffevents log
//! and provides helper functions to serialize and emit them through the tracing
//! infrastructure via [`super::StructuredLogLayer`].

use crate::daemon::{clock_parameters::ClockParameters, receiver_stream::RoutableEvent};

/// Tracing target for the feed-forward events log.
///
/// Used by [`super::StructuredLogLayer`] to route events to `ffevents.log`.
pub const TARGET: &str = "clock_bound::ffevents";

/// File name for the feed-forward events log.
pub const FILE_NAME: &str = "ffevents.log";

/// Emit an `init` event to the ffevents log.
///
/// Called once at daemon startup to delimit log replay boundaries.
#[expect(clippy::missing_panics_doc, reason = "infallible")]
pub fn log_init() {
    let envelope = serde_json::json!({
        "event": "init",
        "init": {
            "message": "Clock Sync Algorithm initialized"
        }
    });
    let entry = serde_json::to_string(&envelope).unwrap();
    tracing::info!(target: TARGET, entry = entry.as_str());
}

/// Emit a `disruption` event to the ffevents log.
///
/// Called when a clock discontinuity is detected and state is reset.
#[expect(clippy::missing_panics_doc, reason = "infallible")]
pub fn log_disruption() {
    let envelope = serde_json::json!({
        "event": "disruption",
        "disruption": {
            "message": "Disruption occurred"
        }
    });
    let entry = serde_json::to_string(&envelope).unwrap();
    tracing::info!(target: TARGET, entry = entry.as_str());
}

/// Emit a `feed` event to the ffevents log containing input and output data.
#[expect(clippy::missing_panics_doc, reason = "infallible")]
pub fn log_feed(input: &RoutableEvent, output: Option<&ClockParameters>) {
    let envelope = serde_json::json!({
        "event": "feed",
        "feed": {
            "input": input,
            "output": output
        }
    });
    let entry = serde_json::to_string(&envelope).unwrap();
    tracing::info!(target: TARGET, entry = entry.as_str());
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use serde_json::Value;
    use tempfile::TempDir;
    use tracing_subscriber::layer::SubscriberExt;

    use crate::daemon::{
        clock_parameters::ClockParameters,
        event::{Ntp, NtpData, Phc, PhcData, Stratum},
        logging::StructuredLogLayer,
        receiver_stream::RoutableEvent,
        time::{Duration, Instant, TscCount, tsc::Period},
    };

    use super::TARGET;

    const TEST_VERSION: &str = "3.0.0-test";

    // --- Helpers ---

    fn sample_ntp_event() -> Ntp {
        Ntp::builder()
            .counter_pre(TscCount::new(100))
            .counter_post(TscCount::new(200))
            .ntp_data(NtpData {
                server_recv_time: Instant::new(1000),
                server_send_time: Instant::new(2000),
                root_delay: Duration::new(50),
                root_dispersion: Duration::new(25),
                stratum: Stratum::ONE,
            })
            .build()
            .unwrap()
    }

    fn sample_phc_event() -> Phc {
        Phc::builder()
            .counter_pre(TscCount::new(300))
            .counter_post(TscCount::new(400))
            .data(PhcData {
                time: Instant::new(5000),
                clock_error_bound: Duration::new(100),
            })
            .build()
            .unwrap()
    }

    fn sample_output() -> ClockParameters {
        ClockParameters {
            tsc_count: TscCount::new(150),
            time: Instant::new(1500),
            clock_error_bound: Duration::new(75),
            period: Period::from_seconds(4.0e-10),
            period_max_error: Period::from_seconds(1.0e-16),
            as_of_monotonic: Instant::new(0),
        }
    }

    /// Set up a StructuredLogLayer targeting ffevents, run a closure, return
    /// the file contents. The `TempDir` is returned to keep it alive for the
    /// caller's scope.
    fn with_ffevents_layer(f: impl FnOnce()) -> (String, TempDir) {
        let tmp_dir = TempDir::new().unwrap();
        let layer = StructuredLogLayer::new(tmp_dir.path(), "ffevents.log", TARGET, TEST_VERSION);

        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, f);

        let mut contents = String::new();
        std::fs::File::open(tmp_dir.path().join("ffevents.log"))
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        (contents, tmp_dir)
    }

    /// Parse output line into Value, compare against expected JSON (copying
    /// the dynamic timestamp from actual into expected before assertion).
    fn assert_matches_expected(actual_line: &str, expected_json: &str) {
        let actual: Value =
            serde_json::from_str(actual_line).expect("actual output is not valid JSON");
        let mut expected: Value =
            serde_json::from_str(expected_json).expect("expected JSON is not valid JSON");

        // Timestamp is dynamic — copy from actual so comparison passes
        expected["timestamp"] = actual["timestamp"].clone();

        assert_eq!(actual, expected);
    }

    // --- End-to-end tests ---

    #[test]
    fn init_event() {
        let (output, _tmp) = with_ffevents_layer(|| {
            super::log_init();
        });

        assert_matches_expected(
            output.lines().next().unwrap(),
            r#"{
                "timestamp": "",
                "event": "init",
                "init": {
                    "message": "Clock Sync Algorithm initialized"
                },
                "version": "3.0.0-test"
            }"#,
        );
    }

    #[test]
    fn disruption_event() {
        let (output, _tmp) = with_ffevents_layer(|| {
            super::log_disruption();
        });

        assert_matches_expected(
            output.lines().next().unwrap(),
            r#"{
                "timestamp": "",
                "event": "disruption",
                "disruption": {
                    "message": "Disruption occurred"
                },
                "version": "3.0.0-test"
            }"#,
        );
    }

    #[test]
    fn feed_ntp_no_output() {
        let (output, _tmp) = with_ffevents_layer(|| {
            let routable = RoutableEvent::AmazonTimeSync(sample_ntp_event());
            super::log_feed(&routable, None);
        });

        assert_matches_expected(
            output.lines().next().unwrap(),
            r#"{
                "timestamp": "",
                "event": "feed",
                "feed": {
                    "input": {
                        "type": "ntp",
                        "identifier": "169.254.169.123:123",
                        "counter_pre": 100,
                        "counter_post": 200,
                        "data": {
                            "server_recv_time_ns": 1000,
                            "server_send_time_ns": 2000,
                            "root_delay_ns": 50,
                            "root_dispersion_ns": 25,
                            "stratum": 1
                        }
                    },
                    "output": null
                },
                "version": "3.0.0-test"
            }"#,
        );
    }

    #[test]
    fn feed_phc_with_output() {
        let output_params = sample_output();
        let (output, _tmp) = with_ffevents_layer(|| {
            let routable = RoutableEvent::Phc(sample_phc_event());
            super::log_feed(&routable, Some(&output_params));
        });

        assert_matches_expected(
            output.lines().next().unwrap(),
            r#"{
                "timestamp": "",
                "event": "feed",
                "feed": {
                    "input": {
                        "type": "phc",
                        "identifier": "/dev/ptp0",
                        "counter_pre": 300,
                        "counter_post": 400,
                        "data": {
                            "time_ns": 5000,
                            "clock_error_bound_ns": 100
                        }
                    },
                    "output": {
                        "counter_sync": 150,
                        "time_sync_ns": 1500,
                        "clock_error_bound_ns": 75,
                        "period_s": 4.0e-10,
                        "period_max_error": 1.0e-16
                    }
                },
                "version": "3.0.0-test"
            }"#,
        );
    }
}
