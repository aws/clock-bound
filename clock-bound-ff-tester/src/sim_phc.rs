//! Code dedicated to generating phc simulations and testing them against clockbound
//!
//! This is a constrained environment where there is only phc data

use clock_bound::daemon::{
    clock_parameters::ClockParameters,
    clock_sync_algorithm::ClockSyncAlgorithm,
    event,
    receiver_stream::RoutableEvent,
    time::{Duration, Instant, tsc::Frequency},
};
use statrs::distribution::Dirac;

use crate::{
    events::{
        Scenario,
        v1::{Event, EventKind},
    },
    sim_amazon_time_sync::NOW_ISH,
    simulation::{
        self,
        delay::{Delay, TimeUnit},
        generator::GeneratorExt,
        oscillator::{FullModel, Oscillator},
        phc::round_trip_delays::VariableRoundTripDelays,
    },
    time::{CbBridge, EstimateDuration, TrueDuration},
};

pub struct TestPhc {
    pub algorithm: ClockSyncAlgorithm,
}

impl TestPhc {
    pub fn new(algorithm: ClockSyncAlgorithm) -> Self {
        Self { algorithm }
    }

    /// Run a whole scenario against a clock sync algorithm
    ///
    /// Returns a list of outputs from the feed fn
    pub fn run(&mut self, scenario: &Scenario) -> Vec<Option<ClockParameters>> {
        let Scenario::V1(scenario) = scenario;
        scenario
            .events
            .iter()
            .map(tester_phc_event_to_cb)
            .map(|e| self.algorithm.feed(e.clone()).cloned())
            .collect()
    }

    pub fn feed_phc(&mut self, event: &Event) -> Option<&ClockParameters> {
        let phc = tester_phc_event_to_cb(event);
        self.algorithm.feed(phc)
    }
}

#[expect(clippy::missing_panics_doc, reason = "unwraps won't panic")]
pub fn perfect_symmetric(scenario_duration: TrueDuration) -> Scenario {
    let oscillator = Oscillator::create_simple()
        .clock_frequency(Frequency::from_ghz(3.3))
        .start_time(NOW_ISH)
        .duration(scenario_duration)
        .call();

    let full_model = FullModel::calculate_from_oscillator(oscillator);

    let network_delays = VariableRoundTripDelays::builder()
        .backward_network(Box::new(Delay::new(
            Dirac::new(2.0).unwrap(),
            TimeUnit::Micros,
        )))
        .forward_network(Box::new(Delay::new(
            Dirac::new(2.0).unwrap(),
            TimeUnit::Micros,
        )))
        .build();

    let clock_error_bound = Box::new(Delay::new(Dirac::new(20.0).unwrap(), TimeUnit::Micros));

    let mut generator = simulation::phc::variable_delay_source::Generator::builder()
        .poll_period(EstimateDuration::from_millis(500))
        .id("phc".into())
        .oscillator(&full_model)
        .network_delays(network_delays)
        .clock_error_bounds(clock_error_bound)
        .build();

    generator.create_scenario(full_model)
}

fn tester_phc_event_to_cb(e: &Event) -> RoutableEvent {
    let EventKind::Phc(p) = &e.variants else {
        panic!("Phc event expected")
    };

    let event = event::Phc::builder()
        .counter_pre(e.client_tsc_pre_time)
        .counter_post(e.client_tsc_post_time)
        .data(event::PhcData {
            time: Instant::from_estimate(p.phc_time),
            clock_error_bound: Duration::from_estimate(p.clock_error_bound.unwrap()),
        })
        .build()
        .unwrap();

    RoutableEvent::Phc(event)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use clock_bound::daemon::{
        clock_sync_algorithm::{
            Selector,
            source::{self, AmazonTimeSync},
        },
        selected_clock::SelectedClockSource,
        time::tsc::Skew,
    };

    use super::*;

    #[test]
    fn perfect_does_not_panic() {
        let skew = Skew::from_ppm(15.0);
        let mut tester = TestPhc::new(
            ClockSyncAlgorithm::builder()
                .amazon_time_sync(AmazonTimeSync::new(skew))
                .ntp_sources(vec![])
                .phc(source::Phc::new("path".into(), skew))
                .selected_clock(Arc::new(SelectedClockSource::default()))
                .selector(Selector::new(skew))
                .build(),
        );

        let scenario = perfect_symmetric(TrueDuration::from_secs(1024));
        let _ = tester.run(&scenario);
    }

    #[test]
    fn alg_is_perfect_under_perfect_conditions() {
        let skew = Skew::from_ppm(15.0);
        let mut tester = TestPhc::new(
            ClockSyncAlgorithm::builder()
                .amazon_time_sync(AmazonTimeSync::new(skew))
                .ntp_sources(vec![])
                .phc(source::Phc::new("path".into(), skew))
                .selected_clock(Arc::new(SelectedClockSource::default()))
                .selector(Selector::new(skew))
                .build(),
        );
        let scenario = perfect_symmetric(TrueDuration::from_secs(1024));
        let Scenario::V1(scenario) = &scenario;
        let mut events = scenario.events.iter().enumerate();
        let (_, first_event) = events.next().unwrap();
        let first_output = tester.feed_phc(first_event);
        assert!(first_output.is_none());

        for (idx, event) in events {
            let output = tester.feed_phc(event);
            let param = output.unwrap();

            let tsc_midpoint = event
                .client_tsc_pre_time
                .midpoint(event.client_tsc_post_time);
            let EventKind::Phc(phc) = &event.variants else {
                panic!("Expected PHC event, found {event:?}");
            };

            assert_eq!(param.tsc_count, tsc_midpoint, "Failure at idx {idx}");
            assert!(
                approx::abs_diff_eq!(
                    param.time.into_estimate().as_nanos(),
                    phc.phc_time.as_nanos(),
                    epsilon = 1
                ),
                "Failure at idx {idx}. parameters not expected: {param:#?}\nexpected {}\ngot{}",
                phc.phc_time.get(),
                param.time.get()
            );
        }
    }
}
