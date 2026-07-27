//! Reproduction utilities

mod legacy_femtoseconds;
mod log_parse;
pub use log_parse::{repro_events_from_log_file, repro_events_from_reader};

use std::{borrow::Borrow, io::Write};

use polars::prelude as pl;

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

impl ReproEvent {
    /// Convert event into a [polars dataframe](pl::DataFrame)
    ///
    /// Schema is generally shaped as a column of `event` structs and optional `clock_params`
    pub fn into_dataframe(events: Vec<Self>) -> Option<pl::DataFrame> {
        use polars::prelude::*;

        let (events, outputs): (Vec<_>, Vec<_>) = events
            .into_iter()
            .filter_map(|e| {
                let Self::Feed { event, output } = e else {
                    return None;
                };
                Some((event, output))
            })
            .unzip();

        let event_df = Self::routable_events_dataframe(&events);
        let params_df = Self::clock_parameters_dataframe(&outputs);

        let event_series = event_df.into_struct("event".into()).into_series();
        let params_series = params_df.into_struct("clock_params".into()).into_series();

        #[expect(clippy::missing_panics_doc, reason = "infallible")]
        let df = DataFrame::new(vec![event_series.into(), params_series.into()]).unwrap();

        // Convert clock_params from a struct filled with nulls to a null struct
        #[expect(clippy::missing_panics_doc, reason = "infallible")]
        let df = df
            .lazy()
            .with_column(
                when(
                    col("clock_params")
                        .struct_()
                        .field_by_name("time")
                        .is_null(),
                )
                .then(lit(Null {}))
                .otherwise(col("clock_params"))
                .alias("clock_params"),
            )
            .collect()
            .unwrap();

        Some(df)
    }

    fn clock_parameters_dataframe(params: &[Option<ClockParameters>]) -> pl::DataFrame {
        use polars::prelude::*;

        let capacity = params.len();
        let mut tsc_counts = Vec::with_capacity(capacity);
        let mut times = Vec::with_capacity(capacity);
        let mut clock_error_bounds = Vec::with_capacity(capacity);
        let mut periods = Vec::with_capacity(capacity);
        let mut period_max_errors = Vec::with_capacity(capacity);
        for param in params {
            let Some(param) = param else {
                tsc_counts.push(None);
                times.push(None);
                clock_error_bounds.push(None);
                periods.push(None);
                period_max_errors.push(None);
                continue;
            };
            tsc_counts.push(Some(param.tsc_count.get()));
            times.push(Some(param.time.as_nanos()));
            clock_error_bounds.push(Some(param.clock_error_bound.as_nanos()));
            periods.push(Some(param.period.get()));
            period_max_errors.push(Some(param.period_max_error.get()));
        }

        let df = df!(
            "tsc_count" => tsc_counts,
            "time" => times,
            "clock_error_bound" => clock_error_bounds,
            "period_s" => periods,
            "period_max_error_s" => period_max_errors,
        )
        .unwrap();

        df.lazy()
            .with_columns([
                Self::cast_col_to_ts_ns("time"),
                Self::cast_col_to_dur_ns("clock_error_bound"),
            ])
            .collect()
            .unwrap()
    }

    fn routable_events_dataframe(events: &[RoutableEvent]) -> pl::DataFrame {
        use polars::prelude::*;
        let capacity = events.len();
        let mut event_types = Vec::with_capacity(capacity);
        let mut tsc_pres = Vec::with_capacity(capacity);
        let mut tsc_posts = Vec::with_capacity(capacity);
        let mut server_recv_times = Vec::with_capacity(capacity);
        let mut server_send_times = Vec::with_capacity(capacity);
        let mut root_delays = Vec::with_capacity(capacity);
        let mut root_dispersions = Vec::with_capacity(capacity);
        let mut stratums = Vec::with_capacity(capacity);
        let mut phc_times = Vec::with_capacity(capacity);
        let mut clock_error_bounds = Vec::with_capacity(capacity);
        let mut socket_addrs = Vec::with_capacity(capacity);

        for event in events {
            match event {
                RoutableEvent::AmazonTimeSync(event) => {
                    event_types.push("AmazonTimeSync".to_string());
                    tsc_pres.push(event.tsc_pre().get());
                    tsc_posts.push(event.tsc_post().get());
                    server_recv_times.push(Some(event.data().server_recv_time.as_nanos()));
                    server_send_times.push(Some(event.data().server_send_time.as_nanos()));
                    root_delays.push(Some(event.data().root_delay.as_nanos()));
                    root_dispersions.push(Some(event.data().root_dispersion.as_nanos()));
                    stratums.push(Some(u8::from(event.data().stratum)));
                    phc_times.push(None);
                    clock_error_bounds.push(None);
                    socket_addrs.push(None);
                }
                RoutableEvent::Phc(event) => {
                    event_types.push("Phc".to_string());
                    tsc_pres.push(event.tsc_pre().get());
                    tsc_posts.push(event.tsc_post().get());
                    server_recv_times.push(None);
                    server_send_times.push(None);
                    root_delays.push(None);
                    root_dispersions.push(None);
                    stratums.push(None);
                    phc_times.push(Some(event.data().time.as_nanos()));
                    clock_error_bounds.push(Some(event.data().clock_error_bound.as_nanos()));
                    socket_addrs.push(None);
                }
                RoutableEvent::NtpSource(addr, event) => {
                    event_types.push("NtpSource".to_string());
                    tsc_pres.push(event.tsc_pre().get());
                    tsc_posts.push(event.tsc_post().get());
                    server_recv_times.push(Some(event.data().server_recv_time.as_nanos()));
                    server_send_times.push(Some(event.data().server_send_time.as_nanos()));
                    root_delays.push(Some(event.data().root_delay.as_nanos()));
                    root_dispersions.push(Some(event.data().root_dispersion.as_nanos()));
                    stratums.push(Some(u8::from(event.data().stratum)));
                    phc_times.push(None);
                    clock_error_bounds.push(None);
                    socket_addrs.push(Some(addr.to_string()));
                }
            }
        }
        let df = df!(
            "type" => event_types,
            "tsc_pre" => tsc_pres,
            "tsc_post" => tsc_posts,
            "server_recv_time" => server_recv_times,
            "server_send_time" => server_send_times,
            "root_delay" => root_delays,
            "root_dispersion" => root_dispersions,
            "stratum" => stratums,
            "phc_time" => phc_times,
            "clock_error_bound" => clock_error_bounds,
            "socket_addr" => socket_addrs,
        )
        .unwrap();

        // convert time types
        df.lazy()
            .with_columns([
                Self::cast_col_to_ts_ns("server_recv_time"),
                Self::cast_col_to_ts_ns("server_send_time"),
                Self::cast_col_to_dur_ns("root_delay"),
                Self::cast_col_to_dur_ns("root_dispersion"),
                Self::cast_col_to_ts_ns("phc_time"),
                Self::cast_col_to_dur_ns("clock_error_bound"),
            ])
            .collect()
            .unwrap()
    }

    fn cast_col_to_ts_ns(col_name: &str) -> pl::Expr {
        use polars::prelude::*;
        col(col_name)
            .cast(DataType::Int64)
            .cast(DataType::Datetime(
                TimeUnit::Nanoseconds,
                Some(TimeZone::UTC),
            ))
            .alias(col_name)
    }

    fn cast_col_to_dur_ns(col_name: &str) -> pl::Expr {
        use polars::prelude::*;
        col(col_name)
            .cast(DataType::Int64)
            .cast(DataType::Duration(TimeUnit::Nanoseconds))
            .alias(col_name)
    }
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
        client_tsc_pre_time: event.tsc_pre(),
        client_tsc_post_time: event.tsc_post(),
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
        client_tsc_pre_time: event.tsc_pre(),
        client_tsc_post_time: event.tsc_post(),
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
                .tsc_pre(event.client_tsc_pre_time)
                .tsc_post(event.client_tsc_post_time)
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
                .tsc_pre(event.client_tsc_pre_time)
                .tsc_post(event.client_tsc_post_time)
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

    #[test]
    fn to_dataframe_does_not_panic() {
        let example_log = include_str!("repro/test_11_06_2025.log");

        let events = repro_events_from_reader(example_log.as_bytes()).unwrap();
        let _df = ReproEvent::into_dataframe(events);
    }
}
