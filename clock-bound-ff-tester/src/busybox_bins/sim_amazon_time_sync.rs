//! Simulation tests against ClockBound's Amazon Time Sync

use std::sync::Arc;

use crate::{
    events::{Scenario, v1::EventKind},
    sim_amazon_time_sync::{self, TestAmazonTimeSync},
    time::{CbBridge, Skew, TrueDuration},
};
use clock_bound::daemon::{
    SharedSelectedClock,
    clock_sync_algorithm::{ClockSyncAlgorithm, Selector, source},
    logging::PRIMER_TARGET,
    selected_clock::SelectedClockSource,
};
use tracing::Level;
use tracing_subscriber::EnvFilter;

pub fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(Level::INFO.into())
                .from_env_lossy()
                .add_directive(format!("{PRIMER_TARGET}=error").parse().unwrap()), // ignore primer
        )
        .init();

    let mut tester = TestAmazonTimeSync::new(
        ClockSyncAlgorithm::builder()
            .amazon_time_sync(source::AmazonTimeSync::new(Skew::from_ppm(15.0)))
            .ntp_sources(vec![])
            .selected_clock(Arc::new(SelectedClockSource::default()))
            .selector(Selector::new(
                Skew::from_ppm(15.0),
                SharedSelectedClock::new(),
            ))
            .build(),
    );

    let scenario = sim_amazon_time_sync::perfect_symmetric(TrueDuration::from_days(14));
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
                param.time.into_estimate().as_nanos(),
                ref_clock_midpoint.as_nanos(),
                epsilon = 1
            ),
            "Failure at idx {idx}. parameters not expected: {param:#?}\nexpected\t{}\ngot\t\t{}",
            ref_clock_midpoint.get(),
            param.time.get()
        );
    }
}
