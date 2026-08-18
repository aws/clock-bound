//! Reproduction utilities

mod log_format_primer;
mod log_parse;
pub use log_parse::{ReproEventReader, repro_events_from_log_file};

use std::{borrow::Borrow, io::Write};

use crate::events::v1;
use crate::time::CbBridge;
use clock_bound::daemon::{
    clock_parameters::ClockParameters,
    event,
    receiver_stream::RoutableEvent,
    time::{Duration, Instant},
};

/// Events fed into the `ClockSyncAlgorithm`
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub enum ReproEvent {
    /// Feed a routable event into the clock sync algorithm
    Feed {
        event: RoutableEvent,
        output: Option<ClockParameters>,
    },
    /// The application initialized.
    Init,
    /// A disruption event came in. Call `handle_disruption`
    Disruption,
}

/// Write a collection of [`ReproEvent`] items into a writeable sink
///
/// Implementation uses the [`serde_json`] implementation on [`ReproEvent`] to write these to newline delimited json.
///
/// The output of this file can be used in other analysis libraries, like with `polars.read_ndjson` in python
///
/// # Errors
/// Returns if there are errors with writing or serialization
pub fn write_repro_events_to_newline_delimited_json<T>(
    events: impl IntoIterator<Item = T>,
    writer: &mut impl Write,
) -> anyhow::Result<()>
where
    T: Borrow<ReproEvent>,
{
    for event in events {
        serde_json::to_writer(writer.by_ref(), event.borrow())?;
        writeln!(writer)?;
    }
    Ok(())
}

pub fn tester_event_from_cb_ntp(event: &event::Ntp, source_id: String) -> v1::Event {
    v1::Event {
        variants: v1::EventKind::Ntp(v1::Ntp {
            server_system_recv_time: event.data().server_recv_time.into_estimate(),
            server_system_send_time: event.data().server_send_time.into_estimate(),
            root_delay: event.data().root_delay.into_estimate(),
            root_dispersion: event.data().root_dispersion.into_estimate(),
            client_system_times: None,
            source_id,
        }),
        client_tsc_pre_time: event.counter_pre(),
        client_tsc_post_time: event.counter_post(),
    }
}

pub fn tester_event_from_cb_phc(event: &event::Phc, source_id: String) -> v1::Event {
    v1::Event {
        variants: v1::EventKind::Phc(v1::Phc {
            phc_time: event.data().time.into_estimate(),
            clock_error_bound: Some(event.data().clock_error_bound.into_estimate()),
            source_id,
            client_system_times: None,
        }),
        client_tsc_pre_time: event.counter_pre(),
        client_tsc_post_time: event.counter_post(),
    }
}

pub fn tester_event_from_routable(event: &RoutableEvent) -> v1::Event {
    match event {
        RoutableEvent::AmazonTimeSync(event) => {
            tester_event_from_cb_ntp(event, String::from("amazon_time_sync"))
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
                .counter_pre(event.client_tsc_pre_time)
                .counter_post(event.client_tsc_post_time)
                .build()
                .unwrap();
            RoutableEvent::AmazonTimeSync(event)
        }
        v1::EventKind::Phc(phc) => {
            let event = event::Phc::builder()
                .data(clock_bound::daemon::event::PhcData {
                    time: Instant::from_estimate(phc.phc_time),
                    clock_error_bound: Duration::from_estimate(phc.clock_error_bound.unwrap()),
                })
                .counter_pre(event.client_tsc_pre_time)
                .counter_post(event.client_tsc_post_time)
                .build()
                .unwrap();
            RoutableEvent::Phc(event)
        }
        v1::EventKind::VMClock(_) => panic!("unsupported"),
    }
}

#[cfg(test)]
mod tests {
    use clock_bound::daemon::{event::Stratum, time::TscCount};

    use crate::time::{EstimateDuration, EstimateInstant};

    use super::log_parse::repro_events_from_reader;
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
            .counter_pre(TscCount::new(500))
            .counter_post(TscCount::new(600))
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
            .counter_pre(TscCount::new(500))
            .counter_post(TscCount::new(600))
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

        let events = repro_events_from_reader(example_log.as_bytes()).unwrap();

        let param_count = events
            .iter()
            .filter_map(|event| {
                let ReproEvent::Feed {
                    event: _event,
                    output: params,
                } = event
                else {
                    return None;
                };
                params.as_ref()
            })
            .count();
        assert_eq!(param_count, 10); // 3 of the values are the first inputs for sources. So 10 calculate clock parameters
    }

    #[test]
    fn scenario_from_nanos_logs() {
        let example_log = include_str!("repro/test_11_06_2025_nanos.log");

        let events = repro_events_from_reader(example_log.as_bytes()).unwrap();

        let param_count = events
            .iter()
            .filter_map(|event| {
                let ReproEvent::Feed {
                    event: _event,
                    output: params,
                } = event
                else {
                    return None;
                };
                params.as_ref()
            })
            .count();
        assert_eq!(param_count, 10); // 3 of the values are the first inputs for sources. So 10 calculate clock parameters
    }
}
