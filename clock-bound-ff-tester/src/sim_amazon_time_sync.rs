//! Code dedicated to generating Amazon Time Sync simulations and testing them against ClockBound
//!
//! This is a constrained environment where there is only Amazon Time Sync data.

use clock_bound::daemon::{
    clock_parameters::ClockParameters,
    clock_sync_algorithm::ClockSyncAlgorithm,
    event,
    receiver_stream::RoutableEvent,
    time::{Duration, Instant, tsc::Frequency},
};

use crate::{
    events::{
        Scenario,
        v1::{Event, EventKind},
    },
    simulation::{
        self,
        generator::GeneratorExt,
        ntp::RoundTripDelays,
        oscillator::{FullModel, Oscillator},
    },
    time::{EstimateDuration, TrueDuration, TrueInstant},
};

pub struct TestAmazonTimeSync {
    pub algorithm: ClockSyncAlgorithm,
}

impl TestAmazonTimeSync {
    pub fn new(algorithm: ClockSyncAlgorithm) -> Self {
        Self { algorithm }
    }

    /// Run a whole scenario against a clock sync algorithm.
    ///
    /// Returns a list of outputs from the inner `feed_amazon_time_sync` function call
    pub fn run(&mut self, scenario: &Scenario) -> Vec<Option<ClockParameters>> {
        let Scenario::V1(scenario) = scenario;
        scenario
            .events
            .iter()
            .map(tester_event_to_amazon_time_sync)
            .map(|e| {
                self.algorithm
                    .feed(e.clone())
                    .map(|sp| sp.clock_parameters.clone())
            })
            .collect()
    }

    pub fn feed_ntp(&mut self, event: &Event) -> Option<&ClockParameters> {
        let ntp = tester_event_to_amazon_time_sync(event);
        self.algorithm.feed(ntp).map(|sp| &sp.clock_parameters)
    }
}

/// Generate a perfect scenario with no ambiguity
///
/// Perfect means:
/// - The TSC oscillator does not deviate from the expected clock frequency (no noise nor systematic drift)
/// - All network traffic is the same and perfectly symmetric
pub fn perfect_symmetric(scenario_duration: TrueDuration) -> Scenario {
    let oscillator = Oscillator::create_simple()
        .clock_frequency(Frequency::from_ghz(3.3))
        .start_time(NOW_ISH)
        .duration(scenario_duration)
        .call();

    let round_trip_delays = RoundTripDelays::builder()
        .server(TrueDuration::from_micros(50))
        .forward_network(TrueDuration::from_micros(38))
        .backward_network(TrueDuration::from_micros(38))
        .build();

    let full_model = FullModel::calculate_from_oscillator(oscillator);

    let mut generator = simulation::ntp::PerfectGenerator::builder()
        .id("ll".into())
        .poll_period(EstimateDuration::from_secs(2))
        .root_delay(EstimateDuration::from_micros(31))
        .root_dispersion(EstimateDuration::from_micros(80))
        .network_delays(round_trip_delays)
        .build();

    generator.create_scenario(full_model)
}

fn tester_event_to_amazon_time_sync(e: &Event) -> RoutableEvent {
    use crate::time::CbBridge;
    let EventKind::Ntp(n) = &e.variants else {
        panic!("Expected NTP event, found {e:?}");
    };

    let event = event::Ntp::builder()
        .counter_pre(e.client_tsc_pre_time)
        .counter_post(e.client_tsc_post_time)
        .ntp_data(event::NtpData {
            server_recv_time: Instant::from_estimate(n.server_system_recv_time),
            server_send_time: Instant::from_estimate(n.server_system_send_time),
            root_delay: Duration::from_estimate(n.root_delay),
            root_dispersion: Duration::from_estimate(n.root_dispersion),
            stratum: event::Stratum::ONE,
        })
        .build()
        .unwrap();

    RoutableEvent::AmazonTimeSync(event)
}

// now-ish. Oct 27 2025
pub const NOW_ISH: TrueInstant = TrueInstant::from_days(365 * 55 + 299);

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use clock_bound::daemon::{
        clock_sync_algorithm::{Selector, source::AmazonTimeSync},
        selected_clock::SelectedClockSource,
        time::tsc::Skew,
    };

    use crate::time::CbBridge;

    use super::*;

    #[test]
    fn perfect_does_not_panic() {
        let mut tester = TestAmazonTimeSync::new(
            ClockSyncAlgorithm::builder()
                .amazon_time_sync(AmazonTimeSync::new(Skew::from_ppm(15.0)))
                .ntp_sources(vec![])
                .selected_clock(Arc::new(SelectedClockSource::default()))
                .selector(Selector::new(Skew::from_ppm(15.0)))
                .build(),
        );
        // running a week long scenario in -O0 is asking for trouble
        let scenario = perfect_symmetric(TrueDuration::from_secs(1024 * 5));
        let _ = tester.run(&scenario);
    }

    #[test]
    fn alg_is_perfect_under_perfect_conditions() {
        let mut tester = TestAmazonTimeSync::new(
            ClockSyncAlgorithm::builder()
                .amazon_time_sync(AmazonTimeSync::new(Skew::from_ppm(15.0)))
                .ntp_sources(vec![])
                .selected_clock(Arc::new(SelectedClockSource::default()))
                .selector(Selector::new(Skew::from_ppm(15.0)))
                .build(),
        );
        let scenario = perfect_symmetric(TrueDuration::from_secs(1024 * 5));
        let Scenario::V1(scenario) = &scenario;
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
                "Failure at idx {idx}. parameters not expected: {param:#?}\nexpected {}\ngot{}",
                ref_clock_midpoint.get(),
                param.time.get()
            );
        }
    }
}
