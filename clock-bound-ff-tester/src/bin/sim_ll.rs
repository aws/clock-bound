//! Simulation tests against ClockBound's Link Local
#![expect(clippy::cast_possible_truncation)]

use std::sync::Arc;

use clock_bound::daemon::{
    clock_sync_algorithm::{ClockSyncAlgorithm, source},
    selected_clock::SelectedClockSource,
    subscriber::PRIMER_TARGET,
};
use clock_bound_ff_tester::{
    events::{Scenario, v1::EventKind},
    sim_ll::{self, TestLinkLocal},
    time::{CbBridge, Skew, TrueDuration},
};
use tracing::Level;
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(Level::INFO.into())
                .from_env_lossy()
                .add_directive(format!("{PRIMER_TARGET}=error").parse().unwrap()), // ignore primer
        )
        .init();

    let mut tester = TestLinkLocal::new(
        ClockSyncAlgorithm::builder()
            .link_local(source::LinkLocal::new(Skew::from_ppm(15.0)))
            .ntp_sources(vec![])
            .selected_clock(Arc::new(SelectedClockSource::default()))
            .build(),
    );

    let scenario = sim_ll::perfect_symmetric(TrueDuration::from_days(14));
    let Scenario::V1(scenario) = scenario;
    let mut events = scenario.events.iter().enumerate();
    let (_, first_event) = events.next().unwrap();

    let first_output = tester.feed_ntp(first_event);
    assert!(first_output.is_none());

    for (idx, event) in events {
        let output = tester.feed_ntp(event);
        let param = output.unwrap();

        let tsc_midpoint = event
            .client_tsc_pre_time
            .midpoint(event.client_tsc_post_time);
        let EventKind::Ntp(ntp) = &event.variants else {
            panic!("Expected NTP event, found {event:?}");
        };
        let ref_clock_midpoint = ntp
            .server_system_recv_time
            .midpoint(ntp.server_system_send_time);

        assert_eq!(param.tsc_count, tsc_midpoint, "Failure at idx {idx}");
        assert!(
            approx::abs_diff_eq!(
                param.time.into_estimate().as_picos() as i64,
                ref_clock_midpoint.as_picos() as i64,
                epsilon = 1000
            ),
            "Failure at idx {idx}. parameters not expected: {param:#?}\nexpected\t{}\ngot\t\t{}",
            ref_clock_midpoint.get(),
            param.time.get()
        );
    }
}
