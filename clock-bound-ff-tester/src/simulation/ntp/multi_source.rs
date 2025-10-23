//! A generator that can be composed of multiple generators to simulate multiple NTP sources.
//!
//! This module provides functionality to combine multiple NTP generators into a single
//! generator that produces events in chronological order, according to the local clock, across all sources.

use std::sync::Mutex;

use crate::events::v1;

/// A generator that combines multiple NTP generators into a single source of events
///
/// This generator handles multiple underlying NTP generators, always selecting
/// the generator with the earliest next event to produce events in chronological order, according
/// to the local clock.
/// The simulation ends when any of the underlying generators is exhausted.
pub struct Generator {
    generators: Vec<Box<dyn super::Generator>>,
    next_generator_index: Mutex<Option<usize>>,
}

impl Generator {
    /// Returns a reference to all underlying generators
    ///
    /// This allows inspection of the component generators without modifying them.
    pub fn generators(&self) -> &[Box<dyn super::Generator>] {
        &self.generators
    }
}

/// Builder for creating a multi-source NTP generator
///
/// Provides a convenient way to add multiple generators and create a combined generator.
#[derive(Default)]
pub struct GeneratorBuilder {
    generators: Vec<Box<dyn super::Generator>>,
}

impl GeneratorBuilder {
    /// Creates a new empty generator builder
    pub fn new() -> Self {
        Self {
            generators: Vec::new(),
        }
    }

    /// Adds a generator to the multi-source generator
    ///
    /// Generators are queried in the order they are added, but events are
    /// produced in chronological order based on their timestamps.
    pub fn add_generator(&mut self, generator: Box<dyn super::Generator>) {
        self.generators.push(generator);
    }

    /// Builds a multi-source generator from the added generators
    ///
    /// Creates a generator that will combine all the added sources.
    /// If no generators have been added, the resulting generator will
    /// always return None from `next_event_ready`.
    pub fn build(self) -> Generator {
        Generator {
            generators: self.generators,
            next_generator_index: Mutex::new(None),
        }
    }
}

impl super::Generator for Generator {
    fn next_event_ready(
        &self,
        oscillator: &crate::simulation::oscillator::FullModel,
    ) -> Option<crate::time::TscCount> {
        let mut soonest_next_event_tstamp = None;
        let mut index_soonest_next_event_generator = None;

        for (i, generator) in self.generators.iter().enumerate() {
            if let Some(next_event_tstamp) = generator.next_event_ready(oscillator) {
                if let Some(tstamp) = soonest_next_event_tstamp {
                    if next_event_tstamp < tstamp {
                        soonest_next_event_tstamp = Some(next_event_tstamp);
                        index_soonest_next_event_generator = Some(i);
                    }
                } else {
                    soonest_next_event_tstamp = Some(next_event_tstamp);
                    index_soonest_next_event_generator = Some(i);
                }
            } else {
                // Simulation is over when any generator is exhausted
                *self.next_generator_index.lock().unwrap() = None;
                return None;
            }
        }

        *self.next_generator_index.lock().unwrap() = index_soonest_next_event_generator;
        soonest_next_event_tstamp
    }

    fn generate(&mut self, oscillator: &crate::simulation::oscillator::FullModel) -> v1::Event {
        if self.next_generator_index.lock().unwrap().is_none() {
            let _ = self.next_event_ready(oscillator);
        }

        let next_generator_index = self
            .next_generator_index
            .get_mut()
            .unwrap()
            .take()
            .expect("All generators should have a generatable next event if next_event_ready.");

        let next_event_generator = &mut self.generators[next_generator_index];
        (*next_event_generator).generate(oscillator)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::events::v1::{Event, EventKind, Ntp};
    use crate::simulation::ntp::{
        DispersionIncreaseLinearGenerator, Generator as NtpGenerator, SeriesGenerator,
        perfect::Generator as PerfectGenerator, round_trip_delays::RoundTripDelays,
    };
    use crate::simulation::oscillator::{FullModel, Oscillator};
    use crate::time::{
        EstimateDuration, EstimateInstant, Frequency, Series, Skew, TrueDuration, TrueInstant,
        TscCount,
    };
    use mockall::{mock, predicate};
    use rstest::{fixture, rstest};

    mock! {
        NtpSource {}
        impl NtpGenerator for NtpSource {
            fn next_event_ready(&self, oscillator: &FullModel) -> Option<TscCount>;
            fn generate(&mut self, oscillator: &FullModel) -> Event;
        }
    }

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
    fn perfect_generator() -> Box<dyn NtpGenerator> {
        Box::new(
            PerfectGenerator::builder()
                .poll_period(EstimateDuration::from_secs(25))
                .id("perfect".to_string())
                .build(),
        )
    }

    #[fixture]
    fn series_generator() -> Box<dyn NtpGenerator> {
        let root_delays: Series<TrueDuration, EstimateDuration> = vec![
            (
                TrueDuration::from_secs(0),
                EstimateDuration::from_micros(100),
            ),
            (
                TrueDuration::from_secs(60),
                EstimateDuration::from_micros(150),
            ),
            (
                TrueDuration::from_secs(120),
                EstimateDuration::from_micros(200),
            ),
        ]
        .into_iter()
        .collect();

        let root_dispersions: Series<TrueDuration, EstimateDuration> = vec![
            (
                TrueDuration::from_secs(0),
                EstimateDuration::from_micros(150),
            ),
            (
                TrueDuration::from_secs(60),
                EstimateDuration::from_micros(200),
            ),
            (
                TrueDuration::from_secs(120),
                EstimateDuration::from_micros(250),
            ),
        ]
        .into_iter()
        .collect();

        let rtds: Series<TrueDuration, RoundTripDelays> = vec![
            (
                TrueDuration::from_secs(0),
                RoundTripDelays::builder()
                    .forward_network(TrueDuration::from_micros(50))
                    .backward_network(TrueDuration::from_micros(50))
                    .server(TrueDuration::from_micros(30))
                    .build(),
            ),
            (
                TrueDuration::from_secs(60),
                RoundTripDelays::builder()
                    .forward_network(TrueDuration::from_micros(60))
                    .backward_network(TrueDuration::from_micros(60))
                    .server(TrueDuration::from_micros(40))
                    .build(),
            ),
            (
                TrueDuration::from_secs(120),
                RoundTripDelays::builder()
                    .forward_network(TrueDuration::from_micros(70))
                    .backward_network(TrueDuration::from_micros(70))
                    .server(TrueDuration::from_micros(50))
                    .build(),
            ),
        ]
        .into_iter()
        .collect();

        Box::new(
            SeriesGenerator::builder()
                .poll_period(EstimateDuration::from_secs(8))
                .id("series".to_string())
                .root_delays(root_delays)
                .root_dispersions(root_dispersions)
                .round_trip_delays(&rtds)
                .build()
                .unwrap(),
        )
    }

    #[fixture]
    fn dispersion_generator() -> Box<dyn NtpGenerator> {
        Box::new(
            DispersionIncreaseLinearGenerator::builder()
                .poll_period(EstimateDuration::from_secs(13))
                .id("dispersion".to_string())
                .dispersion_increase_start_time(TrueDuration::from_minutes(10))
                .dispersion_increase_duration(TrueDuration::from_minutes(30))
                .initial_root_dispersion(EstimateDuration::from_micros(100))
                .max_root_dispersion(EstimateDuration::from_millis(10))
                .build()
                .unwrap(),
        )
    }

    fn create_mock_event(source_id: &str) -> Event {
        Event {
            variants: EventKind::Ntp(Ntp {
                server_system_recv_time: EstimateInstant::from_secs(0),
                server_system_send_time: EstimateInstant::from_secs(0),
                root_delay: EstimateDuration::from_secs(0),
                root_dispersion: EstimateDuration::from_secs(0),
                source_id: source_id.to_string(),
                client_system_times: None,
            }),
            client_tsc_pre_time: TscCount::new(0),
            client_tsc_post_time: TscCount::new(0),
        }
    }

    #[rstest]
    fn builder_with_no_generators(constant_skew_oscillator_model: FullModel) {
        let builder = GeneratorBuilder::new();
        let generator = builder.build();

        assert!(
            generator
                .next_event_ready(&constant_skew_oscillator_model)
                .is_none()
        );
    }

    #[rstest]
    fn builder_with_multiple_generator_types(
        perfect_generator: Box<dyn NtpGenerator>,
        series_generator: Box<dyn NtpGenerator>,
        dispersion_generator: Box<dyn NtpGenerator>,
    ) {
        let mut builder = GeneratorBuilder::new();
        builder.add_generator(perfect_generator);
        builder.add_generator(series_generator);
        builder.add_generator(dispersion_generator);

        let generator = builder.build();
        assert_eq!(generator.generators.len(), 3);
    }

    #[rstest]
    fn generators_accessor(
        perfect_generator: Box<dyn NtpGenerator>,
        series_generator: Box<dyn NtpGenerator>,
        dispersion_generator: Box<dyn NtpGenerator>,
    ) {
        let mut builder = GeneratorBuilder::new();
        builder.add_generator(perfect_generator);
        builder.add_generator(series_generator);
        builder.add_generator(dispersion_generator);

        let generator = builder.build();
        let generators = generator.generators();

        assert_eq!(generators.len(), 3);
    }

    #[rstest]
    fn next_event_ready_chooses_earliest_generator(constant_skew_oscillator_model: FullModel) {
        // Create mock generators with different poll periods
        let mut mock1 = MockNtpSource::new();
        let mut mock2 = MockNtpSource::new();
        let mut mock3 = MockNtpSource::new();

        _ = mock1
            .expect_next_event_ready()
            .with(predicate::always())
            .times(1)
            .return_const(Some(TscCount::new(16_000_000_000)));

        _ = mock2
            .expect_next_event_ready()
            .with(predicate::always())
            .times(1)
            .return_const(Some(TscCount::new(8_000_000_000)));

        _ = mock3
            .expect_next_event_ready()
            .with(predicate::always())
            .times(1)
            .return_const(Some(TscCount::new(12_000_000_000)));

        let mut builder = GeneratorBuilder::new();
        builder.add_generator(Box::new(mock1));
        builder.add_generator(Box::new(mock2));
        builder.add_generator(Box::new(mock3));

        let generator = builder.build();

        // The next event should be from the second generator (8 seconds)
        let next_timestamp = generator.next_event_ready(&constant_skew_oscillator_model);
        assert!(next_timestamp.is_some());
        assert_eq!(next_timestamp.unwrap(), TscCount::new(8_000_000_000));
    }

    #[rstest]
    fn generate_uses_correct_generator_with_mixed_types(
        constant_skew_oscillator_model: FullModel,
        perfect_generator: Box<dyn NtpGenerator>,
        series_generator: Box<dyn NtpGenerator>,
        dispersion_generator: Box<dyn NtpGenerator>,
    ) {
        // Poll periods: perfect=16s, series=8s, dispersion=12s
        let mut builder = GeneratorBuilder::new();
        builder.add_generator(perfect_generator); // poll period 25s
        builder.add_generator(series_generator); // poll period 8s
        builder.add_generator(dispersion_generator); // poll period 13s

        let mut generator = builder.build();

        // The first event should be from series generator (8 seconds)
        let _ = generator.next_event_ready(&constant_skew_oscillator_model);
        let event = generator.generate(&constant_skew_oscillator_model);
        let ntp = event.variants.ntp().unwrap();
        assert_eq!(ntp.source_id, "series");

        // The second event should be from dispersion increase generator (13 seconds)
        let _ = generator.next_event_ready(&constant_skew_oscillator_model);
        let event = generator.generate(&constant_skew_oscillator_model);
        let ntp = event.variants.ntp().unwrap();
        assert_eq!(ntp.source_id, "dispersion");

        // The third event should be from dispersion increase generator (2 x 8 seconds = 16 seconds)
        let _ = generator.next_event_ready(&constant_skew_oscillator_model);
        let event = generator.generate(&constant_skew_oscillator_model);
        let ntp = event.variants.ntp().unwrap();
        assert_eq!(ntp.source_id, "series");
        //
        // The fourth event should be from dispersion increase generator (3 x 8 seconds = 24 seconds)
        let _ = generator.next_event_ready(&constant_skew_oscillator_model);
        let event = generator.generate(&constant_skew_oscillator_model);
        let ntp = event.variants.ntp().unwrap();
        assert_eq!(ntp.source_id, "series");

        // The fifth event should also be from perfect generator (25 seconds)
        let _ = generator.next_event_ready(&constant_skew_oscillator_model);
        let event = generator.generate(&constant_skew_oscillator_model);
        let ntp = event.variants.ntp().unwrap();
        assert_eq!(ntp.source_id, "perfect");

        // The sixth event should be from dispersion increase generator (2 x 13 seconds = 26
        // seconds)
        let _ = generator.next_event_ready(&constant_skew_oscillator_model);
        let event = generator.generate(&constant_skew_oscillator_model);
        let ntp = event.variants.ntp().unwrap();
        assert_eq!(ntp.source_id, "dispersion");
    }

    #[rstest]
    fn exhausted_generator_ends_simulation(constant_skew_oscillator_model: FullModel) {
        // Create mock generators where one returns None
        let mut mock1 = MockNtpSource::new();
        let mut mock2 = MockNtpSource::new();

        // First generator is exhausted
        _ = mock1
            .expect_next_event_ready()
            .with(predicate::always())
            .times(1)
            .return_const(None);

        // Second generator still has events
        _ = mock2.expect_next_event_ready().times(0);

        let mut builder = GeneratorBuilder::new();
        builder.add_generator(Box::new(mock1));
        builder.add_generator(Box::new(mock2));

        let generator = builder.build();

        // next_event_ready should return None because one generator is exhausted
        let next_timestamp = generator.next_event_ready(&constant_skew_oscillator_model);
        assert!(next_timestamp.is_none());
    }

    #[rstest]
    fn generate_calls_next_event_ready_if_needed(constant_skew_oscillator_model: FullModel) {
        let mut mock1 = MockNtpSource::new();
        let mut mock2 = MockNtpSource::new();

        _ = mock1
            .expect_next_event_ready()
            .with(predicate::always())
            .times(1)
            .return_const(Some(TscCount::new(16_000_000_000)));

        _ = mock2
            .expect_next_event_ready()
            .with(predicate::always())
            .times(1)
            .return_const(Some(TscCount::new(8_000_000_000)));

        // Only the second generator should be called to generate
        _ = mock2
            .expect_generate()
            .with(predicate::always())
            .times(1)
            .returning(|_| create_mock_event("mock2"));

        let mut builder = GeneratorBuilder::new();
        builder.add_generator(Box::new(mock1));
        builder.add_generator(Box::new(mock2));

        let mut generator = builder.build();

        // Generate should internally call next_event_ready if next_generator is None
        let event = generator.generate(&constant_skew_oscillator_model);
        let ntp = event.variants.ntp().unwrap();

        // The event should be from the second generator
        assert_eq!(ntp.source_id, "mock2");
    }

    #[rstest]
    fn add_generator_builder_method(
        perfect_generator: Box<dyn NtpGenerator>,
        series_generator: Box<dyn NtpGenerator>,
    ) {
        let mut builder = GeneratorBuilder::new();
        builder.add_generator(perfect_generator);
        builder.add_generator(series_generator);

        let generator = builder.build();
        assert_eq!(generator.generators().len(), 2);
    }
}
