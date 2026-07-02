//! Generator that simulates an NTP server exhibiting a period of increasing dispersion

use crate::simulation::oscillator::FullModel;

use super::{SeriesGenerator, round_trip_delays::RoundTripDelays};
use crate::events::v1::Event;
use crate::time::{EstimateDuration, Series, TrueDuration, TscCount};
use thiserror;

/// Error types specific to dispersion increase scenario configuration
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "Dispersion increase start time must be at least {min:?} into the simulation to allow for at least one poll {poll_period:?} to occur, found {actual:?}."
    )]
    DispersionIncreaseStartTooEarly {
        min: TrueDuration,
        poll_period: EstimateDuration,
        actual: TrueDuration,
    },

    #[error(
        "Dispersion increase duration must be at least {min:?} to span at least one poll period {poll_period:?}, found {actual:?}."
    )]
    DispersionIncreaseDurationTooShort {
        min: TrueDuration,
        poll_period: EstimateDuration,
        actual: TrueDuration,
    },

    #[error(
        "Maximum root dispersion must be greater than initial dispersion, found initial: {initial:?}, max: {max:?}."
    )]
    InvalidDispersionRange {
        initial: EstimateDuration,
        max: EstimateDuration,
    },

    #[error("Series generator error: {0}")]
    SeriesGenerator(#[from] super::series::Error),
}

/// Configuration properties for dispersion increase scenario
#[derive(Debug, Clone)]
pub struct Props {
    /// Time when the server's dispersion starts increasing
    pub dispersion_increase_start_time: TrueDuration,

    /// Duration the dispersion continues to grow
    pub dispersion_increase_duration: TrueDuration,

    /// Root dispersion before increase starts
    pub initial_root_dispersion: EstimateDuration,

    /// Maximum root dispersion
    pub max_root_dispersion: EstimateDuration,

    /// Root delay value
    pub initial_root_delay: EstimateDuration,

    /// Initial round trip delay components
    pub initial_rtt_delays: RoundTripDelays,
}

/// NTP generator that simulates a server experiencing a dispersion increase
pub struct Generator {
    inner: SeriesGenerator,
    id: String,
    props: Props,
}

#[bon::bon]
impl Generator {
    /// Creates a new generator that simulates an NTP server exhibiting a period of root dispersion increase
    ///
    /// # Arguments
    /// * `poll_period` - Time between successive NTP polls
    /// * `id` - Identifier for the NTP source
    /// * `dispersion_increase_start_time` - Time when the root dispersion increase started
    /// * `dispersion_increase_duration` - Duration of root dispersion growth
    /// * `initial_root_dispersion` - Root dispersion before the growth starts
    /// * `max_root_dispersion` - Maximum root dispersion
    /// * `initial_root_delay` - Root delay value (remains constant in this model)
    /// * `initial_rtt_delays` - Round trip delays for forward, server, and backward paths (remains constant in this model)
    ///
    /// # Returns
    /// A Result containing either the configured generator or an Error if validation fails
    #[builder]
    pub fn new(
        poll_period: EstimateDuration,
        id: String,
        #[builder(default = TrueDuration::from_minutes(5))]
        dispersion_increase_start_time: TrueDuration,
        #[builder(default = TrueDuration::from_minutes(30))]
        dispersion_increase_duration: TrueDuration,
        #[builder(default = EstimateDuration::from_micros(100))]
        initial_root_dispersion: EstimateDuration,
        #[builder(default = EstimateDuration::from_millis(10))]
        max_root_dispersion: EstimateDuration,
        #[builder(default = EstimateDuration::from_micros(200))]
        initial_root_delay: EstimateDuration,
        #[builder(default = RoundTripDelays {forward_network: TrueDuration::from_micros(50), server: TrueDuration::from_micros(50), backward_network: TrueDuration::from_micros(50)} )]
        initial_rtt_delays: RoundTripDelays,
    ) -> Result<Self, Error> {
        Generator::validate_dispersion_increase_start_time(
            dispersion_increase_start_time,
            poll_period,
        )?;
        Generator::validate_dispersion_increase_duration(
            dispersion_increase_duration,
            poll_period,
        )?;
        Generator::validate_dispersion_range(initial_root_dispersion, max_root_dispersion)?;

        let props = Props {
            dispersion_increase_start_time,
            dispersion_increase_duration,
            initial_root_dispersion,
            max_root_dispersion,
            initial_root_delay,
            initial_rtt_delays,
        };

        let root_dispersions = Generator::generate_root_dispersion_series(&props);
        let root_delays = Generator::generate_root_delay_series(&props);
        let round_trip_delays = Generator::generate_round_trip_delays_series(&props);

        let inner = match SeriesGenerator::builder()
            .poll_period(poll_period)
            .id(id.clone())
            .root_delays(root_delays)
            .root_dispersions(root_dispersions)
            .round_trip_delays(&round_trip_delays)
            .build()
        {
            Ok(generator) => generator,
            Err(e) => return Err(Error::SeriesGenerator(e)),
        };

        Ok(Self { inner, id, props })
    }

    /// Getter for this generator's source identifier
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Getter for this generator's props
    pub fn props(&self) -> &Props {
        &self.props
    }

    /// Generates a root dispersion series that models a period of dispersion growth
    ///
    /// The series defines the following pattern:
    /// 1. Initial constant root dispersion
    /// 2. Linear increase
    /// 3. Immediate drop
    /// 4. Return to baseline root dispersion
    fn generate_root_dispersion_series(props: &Props) -> Series<TrueDuration, EstimateDuration> {
        // Calculate simulation end time
        let simulation_end = props.dispersion_increase_start_time
            + props.dispersion_increase_duration
            + TrueDuration::from_minutes(5);

        // Define key points in the root dispersion pattern
        let points = vec![
            // Initial baseline dispersion
            (TrueDuration::from_secs(0), props.initial_root_dispersion),
            // Dispersion remains constant until the increase starts
            (
                props.dispersion_increase_start_time,
                props.initial_root_dispersion,
            ),
            // Dispersion reaches maximum
            (
                props.dispersion_increase_start_time + props.dispersion_increase_duration,
                props.max_root_dispersion,
            ),
            // Dispersion drops when the dispersion increase period ends
            (
                props.dispersion_increase_start_time
                    + props.dispersion_increase_duration
                    + TrueDuration::from_secs(1),
                props.initial_root_dispersion,
            ),
            // Continue with reduced dispersion until the end
            (simulation_end, props.initial_root_dispersion),
        ];

        points.into_iter().collect()
    }

    /// Generates a constant root delay series
    fn generate_root_delay_series(props: &Props) -> Series<TrueDuration, EstimateDuration> {
        let simulation_end = props.dispersion_increase_start_time
            + props.dispersion_increase_duration
            + TrueDuration::from_minutes(5);

        let points = vec![
            (TrueDuration::from_secs(0), props.initial_root_delay),
            (simulation_end, props.initial_root_delay),
        ];

        points.into_iter().collect()
    }

    /// Generates a constanct round trip delays series
    fn generate_round_trip_delays_series(props: &Props) -> Series<TrueDuration, RoundTripDelays> {
        let simulation_end = props.dispersion_increase_start_time
            + props.dispersion_increase_duration
            + TrueDuration::from_minutes(5);

        let normal_rtt = RoundTripDelays::builder()
            .forward_network(props.initial_rtt_delays.forward_network)
            .backward_network(props.initial_rtt_delays.backward_network)
            .server(props.initial_rtt_delays.server)
            .build();

        let points = vec![
            (TrueDuration::from_secs(0), normal_rtt.clone()),
            (simulation_end, normal_rtt),
        ];

        points.into_iter().collect()
    }

    fn validate_dispersion_increase_start_time(
        dispersion_increase_start_time: TrueDuration,
        poll_period: EstimateDuration,
    ) -> Result<(), Error> {
        if dispersion_increase_start_time < TrueDuration::from_nanos(poll_period.as_nanos() + 1) {
            return Err(Error::DispersionIncreaseStartTooEarly {
                min: TrueDuration::from_nanos(poll_period.as_nanos() + 1),
                poll_period,
                actual: dispersion_increase_start_time,
            });
        }
        Ok(())
    }

    fn validate_dispersion_increase_duration(
        dispersion_increase_duration: TrueDuration,
        poll_period: EstimateDuration,
    ) -> Result<(), Error> {
        if dispersion_increase_duration < TrueDuration::from_nanos(poll_period.as_nanos() * 2) {
            return Err(Error::DispersionIncreaseDurationTooShort {
                min: TrueDuration::from_nanos(poll_period.as_nanos() * 2),
                poll_period,
                actual: dispersion_increase_duration,
            });
        }
        Ok(())
    }

    fn validate_dispersion_range(
        initial_root_dispersion: EstimateDuration,
        max_root_dispersion: EstimateDuration,
    ) -> Result<(), Error> {
        if max_root_dispersion <= initial_root_dispersion {
            return Err(Error::InvalidDispersionRange {
                initial: initial_root_dispersion,
                max: max_root_dispersion,
            });
        }
        Ok(())
    }
}

impl super::Generator for Generator {
    fn next_event_ready(&self, oscillator: &FullModel) -> Option<TscCount> {
        self.inner.next_event_ready(oscillator)
    }

    fn generate(&mut self, oscillator: &FullModel) -> Event {
        self.inner.generate(oscillator)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::simulation::{ntp::Generator as _, oscillator::Oscillator};
    use crate::time::{Frequency, Skew, TrueInstant};
    use rstest::{fixture, rstest};

    #[fixture]
    fn constant_skew_oscillator_model() -> FullModel {
        let oscillator = Oscillator::create_simple()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(365 * 50))
            .duration(TrueDuration::from_hours(5))
            .tsc_timestamp_start(TscCount::new(10_000))
            .skew(Skew::from_ppm(-10.0))
            .starting_oscillator_offset(TrueDuration::from_micros(200))
            .call();
        FullModel::calculate_from_oscillator(oscillator)
    }

    #[fixture]
    fn poll_period() -> i64 {
        16
    }

    #[fixture]
    fn generator(poll_period: i64) -> Generator {
        Generator::builder()
            .poll_period(EstimateDuration::from_secs(poll_period))
            .id("dispersion_increase_period_test".to_string())
            .build()
            .unwrap()
    }

    #[test]
    fn generator_builder_defaults() {
        let generator = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id("dispersion_increase_period_test".to_string())
            .build()
            .unwrap();

        assert_eq!(generator.id(), "dispersion_increase_period_test");
    }

    #[test]
    fn validation_input_errors() {
        // Test dispersion increase start time is too early
        let result = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id("test".to_string())
            .dispersion_increase_start_time(TrueDuration::from_secs(1))
            .build();

        assert!(matches!(
            result,
            Err(Error::DispersionIncreaseStartTooEarly { .. })
        ));

        // Test dispersion increase duration is too short
        let result = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id("test".to_string())
            .dispersion_increase_duration(TrueDuration::from_secs(1))
            .build();

        assert!(matches!(
            result,
            Err(Error::DispersionIncreaseDurationTooShort { .. })
        ));

        // Test invalid dispersion inputs
        let result = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id("test".to_string())
            .initial_root_dispersion(EstimateDuration::from_millis(10))
            .max_root_dispersion(EstimateDuration::from_micros(100))
            .build();

        assert!(matches!(result, Err(Error::InvalidDispersionRange { .. })));
    }

    #[rstest]
    fn generate_parameter_series(generator: Generator) {
        #[allow(clippy::used_underscore_binding)]
        let props = &generator.props;

        // Test root dispersion series generation
        let root_disp_series = Generator::generate_root_dispersion_series(props);
        assert_eq!(root_disp_series.len(), 5);
        assert_eq!(root_disp_series.data()[0], props.initial_root_dispersion);
        assert_eq!(root_disp_series.data()[1], props.initial_root_dispersion);
        assert_eq!(root_disp_series.data()[2], props.max_root_dispersion);
        assert_eq!(root_disp_series.data()[3], props.initial_root_dispersion);
        assert_eq!(root_disp_series.data()[4], props.initial_root_dispersion);

        // Test root delay series generation
        let root_delay_series = Generator::generate_root_delay_series(props);
        assert_eq!(root_delay_series.len(), 2);
        assert_eq!(root_delay_series.data()[0], props.initial_root_delay);
        assert_eq!(root_delay_series.data()[1], props.initial_root_delay);

        // Test round trip delays series generation
        let rtt_series = Generator::generate_round_trip_delays_series(props);
        assert_eq!(rtt_series.len(), 2);
        assert_eq!(
            rtt_series.data()[0].forward_network,
            props.initial_rtt_delays.forward_network
        );
        assert_eq!(
            rtt_series.data()[1].forward_network,
            props.initial_rtt_delays.forward_network
        );
        assert_eq!(
            rtt_series.data()[0].backward_network,
            props.initial_rtt_delays.backward_network
        );
        assert_eq!(
            rtt_series.data()[1].backward_network,
            props.initial_rtt_delays.backward_network
        );
        assert_eq!(rtt_series.data()[0].server, props.initial_rtt_delays.server);
        assert_eq!(rtt_series.data()[1].server, props.initial_rtt_delays.server);
    }

    #[rstest]
    fn generate_events_before_inrease_during_increase_after_drop(
        constant_skew_oscillator_model: FullModel,
        mut generator: Generator,
    ) {
        // Generate events until we have samples from before increase, during increase, and after drop
        let mut events = Vec::new();
        while generator
            .next_event_ready(&constant_skew_oscillator_model)
            .is_some()
        {
            events.push(generator.generate(&constant_skew_oscillator_model));
        }

        assert!(
            events.len() >= 3,
            "Should have at least 3 events, got {}",
            events.len()
        );

        // Extract root dispersion values from all events
        let dispersions: Vec<EstimateDuration> = events
            .iter()
            .map(|e| {
                let ntp = e.variants.ntp().unwrap();
                ntp.root_dispersion
            })
            .collect();

        let initial_dispersion = dispersions[0];

        let max_dispersion = *dispersions.iter().max().unwrap();

        // Check that we see the expected pattern:
        // 1. Dispersion starts at initial value
        assert_eq!(initial_dispersion, EstimateDuration::from_micros(100));

        // 2. Dispersion increases for a period
        assert!(
            max_dispersion > EstimateDuration::from_nanos(initial_dispersion.as_nanos() * 2),
            "Max dispersion {max_dispersion:?} should be significantly higher than initial {initial_dispersion:?}"
        );

        // 3. Final dispersion should return to initial value
        let final_dispersion = dispersions.last().unwrap();
        let delta = EstimateDuration::from_nanos(
            ((*final_dispersion).as_nanos() - initial_dispersion.as_nanos()).abs(),
        );

        assert!(
            delta < EstimateDuration::from_micros(10),
            "Final dispersion {final_dispersion:?} should be close to initial {initial_dispersion:?}, delta was {delta:?}"
        );
    }
}
