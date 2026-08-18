//! Feed-forward events log (ffevents.log) envelope types and emission helpers.
//!
//! This module defines the typed structures for each event in the ffevents log
//! and provides helper functions to serialize and emit them through the tracing
//! infrastructure via [`super::StructuredLogLayer`].

use serde::Serialize;

use super::{LogEvent, StructuredLog, emit_event_with_seq};
use crate::daemon::{
    clock_parameters::ClockParameters, logging::ffevents::types::ClockParametersLog,
    receiver_stream::RoutableEvent,
};

pub mod types;

/// Body of an `init` event, emitted once at daemon startup to delimit log
/// replay boundaries.
#[derive(Serialize)]
struct Init {
    message: &'static str,
}

impl LogEvent for Init {
    const STREAM: StructuredLog = StructuredLog::FFEvents;
    const EVENT: &'static str = "init";
}

/// Body of a `disruption` event, emitted when a clock discontinuity is detected
/// and state is reset.
#[derive(Serialize)]
struct Disruption {
    message: &'static str,
}

impl LogEvent for Disruption {
    const STREAM: StructuredLog = StructuredLog::FFEvents;
    const EVENT: &'static str = "disruption";
}

/// Body of a `feed` event, carrying the input event and resulting clock output.
#[derive(Serialize)]
struct Feed {
    input: types::RoutableEventLog,
    output: Option<types::ClockParametersLog>,
}

impl LogEvent for Feed {
    const STREAM: StructuredLog = StructuredLog::FFEvents;
    const EVENT: &'static str = "feed";
}

/// Emit an `init` event to the ffevents log.
///
/// Called once at daemon startup to delimit log replay boundaries.
///
/// `seq` is the event sequence number. Callers should start numbering at zero
/// for the `init` event and increment by one for every subsequent `feed` or
/// `disruption` event.
pub(crate) fn log_init(seq: u64) {
    emit_event_with_seq(
        &Init {
            message: "Clock Sync Algorithm initialized",
        },
        seq,
    );
}

/// Emit a `disruption` event to the ffevents log.
///
/// Called when a clock discontinuity is detected and state is reset.
///
/// `seq` is the event sequence number. See [`log_init`] for numbering rules.
pub(crate) fn log_disruption(seq: u64) {
    emit_event_with_seq(
        &Disruption {
            message: "Disruption occurred",
        },
        seq,
    );
}

/// Emit a `feed` event to the ffevents log containing input and output data.
///
/// `seq` is the event sequence number. See [`log_init`] for numbering rules.
pub(crate) fn log_feed(input: &RoutableEvent, output: Option<&ClockParameters>, seq: u64) {
    emit_event_with_seq(
        &Feed {
            input: input.into(),
            output: output.map(ClockParametersLog::from),
        },
        seq,
    );
}

#[cfg(test)]
mod tests {
    use crate::daemon::{
        clock_parameters::ClockParameters,
        event::{Ntp, NtpData, Phc, PhcData, Stratum},
        logging::{
            StructuredLog,
            test_support::{assert_matches_expected, with_layer},
        },
        receiver_stream::RoutableEvent,
        time::{Duration, Instant, TscCount, tsc::Period},
    };

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
    fn with_ffevents_layer(f: impl FnOnce()) -> (String, tempfile::TempDir) {
        with_layer(StructuredLog::FFEvents, TEST_VERSION, f)
    }

    // --- End-to-end tests ---

    #[test]
    fn init_event() {
        let (output, _tmp) = with_ffevents_layer(|| {
            super::log_init(0);
        });

        assert_matches_expected(
            output.lines().next().unwrap(),
            r#"{
                "timestamp": "",
                "event": "init",
                "seq": 0,
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
            super::log_disruption(7);
        });

        assert_matches_expected(
            output.lines().next().unwrap(),
            r#"{
                "timestamp": "",
                "event": "disruption",
                "seq": 7,
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
            super::log_feed(&routable, None, 1);
        });

        assert_matches_expected(
            output.lines().next().unwrap(),
            r#"{
                "timestamp": "",
                "event": "feed",
                "seq": 1,
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
            super::log_feed(&routable, Some(&output_params), 2);
        });

        assert_matches_expected(
            output.lines().next().unwrap(),
            r#"{
                "timestamp": "",
                "event": "feed",
                "seq": 2,
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
                        "period_max_error_s": 1.0e-16
                    }
                },
                "version": "3.0.0-test"
            }"#,
        );
    }
}
