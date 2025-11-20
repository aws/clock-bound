//! Convert logs into `ff-tester` types in a Scenario

use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, Read},
    path::Path,
};

use crate::events::{Scenario, v1};
use crate::time::CbBridge;
use clock_bound::daemon::{
    clock_parameters::ClockParameters,
    event,
    receiver_stream::RoutableEvent,
    time::{Duration, Instant},
};

/// Read a logfile and return all inputs and outputs from the `ClockSyncAlgorithm`
///
/// # Errors
/// - Returns an error if unable to open the file.
/// - Returns and error if unable to read values from the logfile
pub fn scenario_from_log_file(
    file_path: impl AsRef<Path>,
) -> anyhow::Result<(Scenario, Vec<Option<ClockParameters>>)> {
    let file = File::open(file_path)?;
    scenario_from_reader(file)
}

/// Convert a log file at `file_path` into a `ff-tester` scenario
///
/// Returns a Scenario, and the output from every call into `ClockSyncAlgorithm::feed_link_local`
///
/// FIXME: Need to fix source id when integrating `routing` into the `ClockSyncAlgorithm`
///
/// # Errors
/// Returns errors if the logfile is corrupted
pub fn scenario_from_reader(
    input: impl Read,
) -> anyhow::Result<(Scenario, Vec<Option<ClockParameters>>)> {
    let reader = std::io::BufReader::new(input);

    let mut events = Vec::new();
    let mut clock_parameters = Vec::new();

    for line in reader.lines() {
        let Ok(line) = line else {
            tracing::warn!("Could not read line from log file");
            continue;
        };
        let line: Line = match serde_json::from_str(&line) {
            Ok(line) => line,
            Err(e) => {
                tracing::warn!(line, "Could not parse line from log file: {e}");
                continue;
            }
        };

        let event = match line.fields.parse_event() {
            Ok(event) => event,
            Err(e) => {
                tracing::warn!(line = ?line, "Could not parse event: {e}");
                continue;
            }
        };

        let output = match line.fields.parse_output() {
            Ok(output) => output,
            Err(e) => {
                tracing::warn!(line = ?line, "Could not parse output: {e}");
                continue;
            }
        };

        events.push(event);
        clock_parameters.push(output);
    }

    if events.is_empty() {
        anyhow::bail!("No events found in log file");
    }

    if clock_parameters.is_empty() {
        anyhow::bail!("No clock parameters found in log file");
    }

    if events.len() != clock_parameters.len() {
        anyhow::bail!("Number of events and clock parameters do not match");
    }

    let events: Vec<_> = events
        .into_iter()
        .map(|event| tester_event_from_routable(&event))
        .collect();

    let scenario = Scenario::V1(v1::Scenario {
        events,
        oscillator: None,
        metadata: HashMap::new(),
    });

    Ok((scenario, clock_parameters))
}

fn tester_event_from_cb_ntp(event: &event::Ntp, source_id: String) -> v1::Event {
    v1::Event {
        variants: v1::EventKind::Ntp(v1::Ntp {
            server_system_recv_time: event.data().server_recv_time.into_estimate(),
            server_system_send_time: event.data().server_send_time.into_estimate(),
            root_delay: event.data().root_delay.into_estimate(),
            root_dispersion: event.data().root_dispersion.into_estimate(),
            client_system_times: None,
            source_id,
        }),
        client_tsc_pre_time: event.tsc_pre(),
        client_tsc_post_time: event.tsc_post(),
    }
}

fn tester_event_from_cb_phc(event: &event::Phc, source_id: String) -> v1::Event {
    v1::Event {
        variants: v1::EventKind::Phc(v1::Phc {
            phc_time: event.data().time.into_estimate(),
            clock_error_bound: Some(event.data().clock_error_bound.into_estimate()),
            source_id,
            client_system_times: None,
        }),
        client_tsc_pre_time: event.tsc_pre(),
        client_tsc_post_time: event.tsc_post(),
    }
}

fn tester_event_from_routable(event: &RoutableEvent) -> v1::Event {
    match event {
        RoutableEvent::LinkLocal(event) => {
            tester_event_from_cb_ntp(event, String::from("link_local"))
        }
        RoutableEvent::NtpSource(addr, event) => tester_event_from_cb_ntp(event, addr.to_string()),
        RoutableEvent::Phc(event) => tester_event_from_cb_phc(event, String::from("ptp_ena")),
    }
}

#[expect(clippy::missing_panics_doc, reason = "malformed input otherwise")]
pub fn routable_from_tester_event(event: v1::Event) -> RoutableEvent {
    match event.variants {
        v1::EventKind::Ntp(ntp) => {
            let event = event::Ntp::builder()
                .ntp_data(clock_bound::daemon::event::NtpData {
                    server_recv_time: Instant::from_estimate(ntp.server_system_recv_time),
                    server_send_time: Instant::from_estimate(ntp.server_system_send_time),
                    root_delay: Duration::from_estimate(ntp.root_delay),
                    root_dispersion: Duration::from_estimate(ntp.root_dispersion),
                    stratum: clock_bound::daemon::event::Stratum::TWO,
                })
                .tsc_pre(event.client_tsc_pre_time)
                .tsc_post(event.client_tsc_post_time)
                .build()
                .unwrap();
            RoutableEvent::LinkLocal(event)
        }
        v1::EventKind::Phc(phc) => {
            let event = event::Phc::builder()
                .data(clock_bound::daemon::event::PhcData {
                    time: Instant::from_estimate(phc.phc_time),
                    clock_error_bound: Duration::from_estimate(phc.clock_error_bound.unwrap()),
                })
                .tsc_pre(event.client_tsc_pre_time)
                .tsc_post(event.client_tsc_post_time)
                .build()
                .unwrap();
            RoutableEvent::Phc(event)
        }
        v1::EventKind::VMClock(_) => panic!("unsupported"),
    }
}

/// Convenience struct to parse each line of the log
///
/// It's not exhaustive, but has the minimum number of fields
/// to get the [`NTP event`](clock_bound::daemon::event::Ntp) and [`ClockParameters`]
#[derive(Debug, Clone, serde::Deserialize)]
struct Line {
    fields: Fields,
}

/// Workaround for tracing escapes
///
/// Holds strings to make it easy to grab the inner keys. But since these values cannot be parsed directly from json,
/// implement methods to parse each event.
///
/// tracing currently escapes double quote characters in json when logging. So we need to convert all `\"` into '"'
#[derive(Debug, Clone, serde::Deserialize)]
struct Fields {
    event: String, // FIXME switch to routable event
    output: String,
}

impl Fields {
    fn parse_output(&self) -> anyhow::Result<Option<ClockParameters>> {
        let unescaped = self.output.replace("\\\"", "\"");
        let retval: Option<ClockParameters> = serde_json::from_str(&unescaped)?;
        Ok(retval)
    }

    fn parse_event(&self) -> anyhow::Result<RoutableEvent> {
        let unescaped = self.event.replace("\\\"", "\"");
        let retval: RoutableEvent = serde_json::from_str(&unescaped)?;
        Ok(retval)
    }
}

#[cfg(test)]
mod tests {
    use clock_bound::daemon::{event::Stratum, time::TscCount};

    use crate::time::{EstimateDuration, EstimateInstant};

    use super::*;

    #[test]
    fn convert_ntp_clock_bound_event_to_tester() {
        let event = event::Ntp::builder()
            .ntp_data(clock_bound::daemon::event::NtpData {
                server_recv_time: 1.into(),
                server_send_time: 2.into(),
                root_delay: 3.into(),
                root_dispersion: 4.into(),
                stratum: Stratum::TWO,
            })
            .tsc_pre(TscCount::new(500))
            .tsc_post(TscCount::new(600))
            .build()
            .unwrap();

        let source_id = "source_id".to_string();
        let tester_event = tester_event_from_cb_ntp(&event, source_id.clone());
        assert_eq!(tester_event.client_tsc_pre_time, TscCount::new(500));
        assert_eq!(tester_event.client_tsc_post_time, TscCount::new(600));
        assert_eq!(
            tester_event.variants,
            v1::EventKind::Ntp(v1::Ntp {
                server_system_recv_time: EstimateInstant::new(1),
                server_system_send_time: EstimateInstant::new(2),
                root_delay: EstimateDuration::new(3),
                root_dispersion: EstimateDuration::new(4),
                client_system_times: None,
                source_id,
            })
        );
    }

    #[test]
    fn phc_conversion() {
        let event = event::Phc::builder()
            .data(event::PhcData {
                time: 1.into(),
                clock_error_bound: 2.into(),
            })
            .tsc_pre(TscCount::new(500))
            .tsc_post(TscCount::new(600))
            .build()
            .unwrap();

        let source_id = "source_id".to_string();
        let tester_event = tester_event_from_cb_phc(&event, source_id.clone());
        assert_eq!(tester_event.client_tsc_pre_time, TscCount::new(500));
        assert_eq!(tester_event.client_tsc_post_time, TscCount::new(600));
        assert_eq!(
            tester_event.variants,
            v1::EventKind::Phc(v1::Phc {
                phc_time: EstimateInstant::new(1),
                clock_error_bound: Some(EstimateDuration::new(2)),
                source_id,
                client_system_times: None,
            })
        );
    }

    #[test]
    fn scenario_from_logs() {
        let example_log = include_str!("repro/test_11_06_2025.log");

        let (scenario, clock_parameters) = scenario_from_reader(example_log.as_bytes()).unwrap();

        assert_eq!(clock_parameters.len(), 13);
        let Scenario::V1(scenario) = scenario;
        assert_eq!(scenario.events.len(), 13);
        assert!(scenario.oscillator.is_none());

        let param_count = clock_parameters
            .iter()
            .filter_map(|params| params.as_ref())
            .count();
        assert_eq!(param_count, 10); // 3 of the values are the first inputs for sources. So 10 calculate clock parameters
    }
}
