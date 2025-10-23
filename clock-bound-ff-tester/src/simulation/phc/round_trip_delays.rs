//! PHC round trip delay

use super::variable_delay_source::PhcEventTimestamps;
use crate::simulation::{
    delay::DelayRng, interpolation::SeriesInterpolation, oscillator::FullModel,
};
use crate::time::{EstimateDuration, TrueDuration};

/// Building block for taking a set of network delays and creating PHC events
#[derive(Debug, Clone, Default)]
pub struct RoundTripDelays {
    /// the true time delay of the PHC request from the client to the server
    pub forward_network: TrueDuration,
    /// the true time delay of the PHC reply back from the server to the client
    pub backward_network: TrueDuration,
}

impl RoundTripDelays {
    pub fn calculate_phc_event_timestamps(
        &self,
        oscillator_estimated_send_delay: EstimateDuration,
        oscillator: &FullModel,
        clock_error_bound: Option<EstimateDuration>,
    ) -> Option<PhcEventTimestamps> {
        // This algorithm works well by converting between the TSC domain to the TrueTime domain and back again.
        //
        // While this may *seem* roundabout, it's a simple way to understand the problem. Convert your start time into the True Time domain,
        // do your operations, and then convert back.

        // First we need the tsc timestamp at the client send time
        let client_send = oscillator_estimated_send_delay
            * oscillator.oscillator().clock_frequency()
            + oscillator.oscillator().tsc_timestamp_start();

        // Then, convert this tsc timestamp into a true time with a reverse linear interpolation
        let client_send_true = oscillator
            .true_time_to_tsc_timestamps()
            .as_ref()
            .reverse_approximate(client_send)?;

        // then add network and server delays
        let phc_time = client_send_true + self.forward_network;
        let client_recv = phc_time + self.backward_network;

        // then convert back to the tsc timestamp domain
        let client_recv = oscillator
            .true_time_to_tsc_timestamps()
            .as_ref()
            .approximate(client_recv)?;

        Some(PhcEventTimestamps {
            client_send,
            phc_time,
            client_recv,
            clock_error_bound,
        })
    }
}
/// Building block for taking a set of network delays and creating PHC events
#[derive(bon::Builder, Debug)]
pub struct VariableRoundTripDelays {
    /// the true time delay of the PHC request from the client to the server
    pub forward_network: Box<dyn DelayRng>,
    /// the true time delay of the PHC reply back from the server to the client
    pub backward_network: Box<dyn DelayRng>,
}

impl VariableRoundTripDelays {
    pub fn generate_round_trip_delays(
        &self,
        rng: &mut rand_chacha::ChaCha12Rng,
    ) -> RoundTripDelays {
        RoundTripDelays {
            forward_network: self.forward_network.get_value(rng),
            backward_network: self.backward_network.get_value(rng),
        }
    }

    /// Contains placeholder values.
    /// Do not use this function if you need good, well informed, initial values.
    #[expect(
        clippy::missing_panics_doc,
        reason = "Gamma Parameters are constant and won't panic"
    )]
    pub fn test_default() -> Self {
        use crate::simulation::{
            delay::{Delay, TimeUnit},
            stats,
        };

        VariableRoundTripDelays::builder()
            .forward_network(Box::new(Delay::new(
                stats::GammaDistribution::new(0.1, 1.0, 0.0).unwrap(),
                TimeUnit::Micros,
            )))
            .backward_network(Box::new(Delay::new(
                stats::GammaDistribution::new(0.1, 1.0, 0.0).unwrap(),
                TimeUnit::Micros,
            )))
            .build()
    }
}

#[cfg(test)]
mod test {
    use crate::time::{AssumeTrue, Frequency, Skew, TrueInstant, TscCount};
    use rstest::{fixture, rstest};

    use crate::simulation::oscillator::{FullModel, Oscillator};

    use super::*;

    #[fixture]
    fn perfect_oscillator() -> FullModel {
        // A perfect oscillator with a 1GHz frequency is a perfect monotonic tsc clock (aka 1 tick vs nanosecond)
        // This makes math a lot easier for the basic case
        let oscillator = Oscillator::create_simple()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(365 * 50))
            .duration(TrueDuration::from_hours(5))
            .call();
        FullModel::calculate_from_oscillator(oscillator)
    }

    #[fixture]
    fn positive_skew_oscillator() -> FullModel {
        // A positive skew oscillator with a 1GHz frequency is a perfect monotonic tsc clock (aka 1 tick vs nanosecond)
        // This makes math a lot easier for the basic case
        let oscillator = Oscillator::create_simple()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(365 * 50))
            .duration(TrueDuration::from_hours(5))
            .skew(Skew::from_percent(5.0)) // wow now thats I call a bad oscillator
            .call();
        FullModel::calculate_from_oscillator(oscillator)
    }

    #[rstest]
    fn calculate_phc_event_timestamps_perfect(perfect_oscillator: FullModel) {
        // expectations
        //
        // The oscillator has no offset, as such we should be able to accurately calculate the
        // timestamps.

        let start_time = perfect_oscillator.oscillator().start_time();
        let forward_network = TrueDuration::from_micros(50);
        let backward_network = TrueDuration::from_micros(55);
        let client_start = EstimateDuration::from_millis(100);

        let delays = RoundTripDelays {
            forward_network,
            backward_network,
        };

        let event = delays.calculate_phc_event_timestamps(client_start, &perfect_oscillator, None);

        assert_eq!(
            event,
            Some(PhcEventTimestamps {
                client_send: TscCount::new(client_start.as_nanos()),
                phc_time: client_start.assume_true() + start_time + forward_network,
                client_recv: TscCount::new(
                    (client_start.assume_true() + forward_network + backward_network).as_nanos()
                ),
                clock_error_bound: None,
            })
        );
    }

    #[rstest]
    fn calculate_phc_event_timestamps_with_ceb(perfect_oscillator: FullModel) {
        // expectations
        //
        // The oscillator has no offset, as such we should be able to accurately calculate the
        // timestamps.

        let start_time = perfect_oscillator.oscillator().start_time();
        let forward_network = TrueDuration::from_micros(50);
        let backward_network = TrueDuration::from_micros(55);
        let client_start = EstimateDuration::from_millis(100);
        let clock_error_bound = Some(EstimateDuration::from_millis(10));

        let delays = RoundTripDelays {
            forward_network,
            backward_network,
        };

        let event = delays.calculate_phc_event_timestamps(
            client_start,
            &perfect_oscillator,
            clock_error_bound,
        );

        assert_eq!(
            event,
            Some(PhcEventTimestamps {
                client_send: TscCount::new(client_start.as_nanos()),
                phc_time: client_start.assume_true() + start_time + forward_network,
                client_recv: TscCount::new(
                    (client_start.assume_true() + forward_network + backward_network).as_nanos()
                ),
                clock_error_bound,
            })
        );
    }

    #[rstest]
    fn calculate_phc_event_timestamps_positive_skew(positive_skew_oscillator: FullModel) {
        // expectations
        //
        // The generator in this test has a 1GHz clock, which matches a 1 nanosecond interval in
        // `ff-tester` time types. This means that a skew should be easy to test with comparison operators
        let start_time = positive_skew_oscillator.oscillator().start_time();
        let forward_network = TrueDuration::from_micros(50);
        let backward_network = TrueDuration::from_micros(55);
        let client_start = EstimateDuration::from_millis(100);

        let delays = RoundTripDelays {
            forward_network,
            backward_network,
        };

        let event =
            delays.calculate_phc_event_timestamps(client_start, &positive_skew_oscillator, None);

        let timestamps = event.unwrap();

        // the client_send tsc timestamp and client_start estimate time should agree.
        assert_eq!(
            timestamps.client_send,
            TscCount::new(client_start.as_nanos())
        );

        // Because the oscillator is skewed fast, the PHC time represents true time.
        // This inequality shows that the true time is actually earlier than the normal calculations would expect
        assert!(timestamps.phc_time < start_time + client_start.assume_true() + forward_network);
        assert!(timestamps.phc_time < start_time + client_start.assume_true() + forward_network);

        // The corollary to above: when the reply comes back, the local oscillator increased at a faster rate than the true time during the time it took to get time from the PHC.
        assert!(
            timestamps.client_recv
                > TscCount::new(
                    (client_start.assume_true() + forward_network + backward_network).as_nanos()
                )
        );
    }
}
