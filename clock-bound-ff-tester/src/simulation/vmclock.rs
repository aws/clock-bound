//! VMClock related generators and support

use std::fmt::Debug;

use crate::events::v1::{Event, EventKind, VMClock};
use crate::time::{DemoteToEstimate, EstimateDuration, Period, TscDiff};
use rand::RngCore;

use super::{
    delay::{DelayRng, TimeUnit},
    interpolation::SeriesInterpolation,
    oscillator::FullModel,
};

#[derive(Debug)]
pub struct Props {
    /// The rate at which the VMClock changes
    ///
    /// This value is not like other poll periods, in that a real system will likely poll the
    /// VMClock at a MUCH faster rate (thinking 100x a second). This value will reflect the
    /// rate at which the VMClock may realistically change
    pub update_period: EstimateDuration,
    /// The maximum error of the frequency estimate
    pub clock_period_max_error: Option<Box<dyn DelayRng>>,
    /// The amount of time in the past that the VMClock was updated
    pub vmclock_time_lag: Box<dyn DelayRng>,
}

/// A VMClock-only generator
///
/// # Generation logic
///
/// This struct generally generates a new event every `update_period` period of time.
///
/// ## Clock Period
/// It calculates the `clock_period` with 100% accuracy on each generations. However, the clock frequency max error field
/// adds potential ranges of error estimates.
///
/// ## VMClock Time
/// The current implementation has a *perfect* `vmclock_time` accuracy, and NO clock error bound reading. The
/// clock sync algorithm must handle this case. The VMClock time will be some value in the past of the `next_event`,
/// randomized with a `DelayRng`
pub struct Generator {
    props: Props,
    id: String,
    next_poll: EstimateDuration,
    next_event: Option<Event>,
    rng: Box<dyn RngCore>,
}

impl Debug for Generator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Generator")
            .field("props", &self.props)
            .field("id", &self.id)
            .field("next_poll", &self.next_poll)
            .field("next_event", &self.next_event)
            .finish_non_exhaustive()
    }
}

#[bon::bon]
impl Generator {
    /// Constructor
    #[builder]
    pub fn new(props: Props, id: String, oscillator: &FullModel, rng: Box<dyn RngCore>) -> Self {
        let mut rv = Self {
            id,
            next_poll: props.update_period,
            props,
            next_event: None,
            rng,
        };

        let next_event = rv.next_event(oscillator);
        rv.next_event = next_event;

        rv
    }
}

impl Generator {
    /// Generate the next event
    fn next_event(&mut self, full_model: &FullModel) -> Option<Event> {
        let tsc_start = self.next_poll * full_model.oscillator().clock_frequency()
            + full_model.oscillator().tsc_timestamp_start();

        // tsc values aren't REALLY used. Just put vaguely logical values.
        let tsc_end = tsc_start + TscDiff::new(100);

        // Get the time in the past that this event occurred
        let true_tsc_start_time = full_model
            .true_time_to_tsc_timestamps()
            .as_ref()
            .reverse_approximate(tsc_start)?; // returns none if this is past the end of scenario
        let lag = self.props.vmclock_time_lag.get_value(&mut self.rng);
        let vmclock_time = true_tsc_start_time - lag;

        // now get the corresponding tsc value for vmclock_time
        let (vmclock_tsc, vmclock_time) = if let Some(tsc) = full_model
            .true_time_to_tsc_timestamps()
            .as_ref()
            .approximate(vmclock_time)
        {
            (tsc, vmclock_time)
        } else {
            // If we are going WAAAY too far in the past, just clamp for now
            let (time, tsc) = full_model
                .true_time_to_tsc_timestamps()
                .as_ref()
                .iter()
                .next()
                .unwrap();
            (*tsc, *time)
        };

        // the time since the start of the scenario
        let true_vmclock_duration = vmclock_time - full_model.oscillator().start_time();

        // lets use it to get the clock frequency/period of this time
        let frequency = full_model
            .frequencies()
            .approximate(true_vmclock_duration)
            .expect("frequency value should exist at this point");

        let period = frequency.period();
        let period_max_error = self.props.clock_period_max_error.as_ref().map(|d| {
            let (delay, unit) = d.get_value_tsc(&mut self.rng);
            match unit {
                TimeUnit::Secs => Period::from_seconds(delay),
                TimeUnit::Millis => Period::from_seconds(delay / 1.0e3),
                TimeUnit::Micros => Period::from_seconds(delay / 1.0e6),
                TimeUnit::Nanos => Period::from_seconds(delay / 1.0e9),
            }
        });

        Some(Event {
            client_tsc_pre_time: tsc_start,
            client_tsc_post_time: tsc_end,
            variants: EventKind::VMClock(VMClock {
                tsc_timestamp: vmclock_tsc,
                vmclock_time: vmclock_time.demote_to_estimate(),
                vmclock_time_max_error: None, // unsupported atm
                clock_period: period,
                clock_period_max_error: period_max_error,
                source_id: self.id.clone(),
            }),
        })
    }
}

impl crate::simulation::generator::Generator for Generator {
    fn next_event_ready(&self, oscillator: &FullModel) -> Option<crate::time::TscCount> {
        if self.next_event.is_some() {
            let true_dur = oscillator.oscillator_estimate_to_true_duration(self.next_poll)?;
            let true_time = oscillator.oscillator().start_time() + true_dur;
            let tsc_start = oscillator
                .true_time_to_tsc_timestamps()
                .as_ref()
                .approximate(true_time)?;
            Some(tsc_start)
        } else {
            None
        }
    }

    fn generate(&mut self, oscillator: &FullModel) -> Event {
        let event = self.next_event.take().unwrap();

        // update next poll time
        self.next_poll += self.props.update_period;

        // generate next event
        self.next_event = self.next_event(oscillator);

        event
    }
}

#[cfg(test)]
mod test {
    use crate::simulation::{delay::Delay, oscillator::Oscillator};

    use super::*;
    use crate::simulation::generator::Generator as _;
    use crate::time::{AssumeTrue, Frequency, Skew, TrueDuration, TrueInstant, TscCount};
    use rand::SeedableRng;
    use rand_chacha::ChaCha12Rng;
    use rstest::{fixture, rstest};
    use statrs::distribution::Dirac;

    #[fixture]
    fn constant_skew() -> FullModel {
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
    fn dut_generator(constant_skew: FullModel) -> Generator {
        let props = Props {
            update_period: EstimateDuration::from_secs(50),
            clock_period_max_error: Some(Box::new(Delay::new(
                Dirac::new(0.3).unwrap(),
                TimeUnit::Nanos,
            ))),
            vmclock_time_lag: Box::new(Delay::new(Dirac::new(1.0).unwrap(), TimeUnit::Secs)),
        };

        Generator::builder()
            .props(props)
            .id(String::from("test"))
            .oscillator(&constant_skew)
            .rng(Box::new(ChaCha12Rng::from_seed(Default::default())))
            .build()
    }

    #[rstest]
    fn generator_initialization(dut_generator: Generator) {
        assert_eq!(dut_generator.id, "test");
        assert_eq!(dut_generator.next_poll, EstimateDuration::from_secs(50));
        assert!(dut_generator.next_event.is_some());
    }

    #[rstest]
    fn next_event_ready(dut_generator: Generator, constant_skew: FullModel) {
        let ready_time = dut_generator.next_event_ready(&constant_skew);
        assert!(ready_time.is_some());
    }

    // Kind of a regression value test with TSC value
    #[rstest]
    fn generate_event(mut dut_generator: Generator, constant_skew: FullModel) {
        let event = dut_generator.generate(&constant_skew);

        match event.variants {
            EventKind::VMClock(vmclock) => {
                assert_eq!(vmclock.source_id, "test");
                assert_eq!(vmclock.tsc_timestamp, TscCount::new(49_000_020_000));
                approx::assert_abs_diff_eq!(vmclock.clock_period.get(), 1.000_010e-9);
                // Verify clock_period_max_error is present and matches our fixture setup
                approx::assert_abs_diff_eq!(vmclock.clock_period_max_error.unwrap().get(), 3e-10);
            }
            _ => panic!("Expected VMClock event"),
        }

        // Verify next event is generated
        assert!(dut_generator.next_event.is_some());

        // Verify next_poll was updated
        assert_eq!(
            dut_generator.next_poll,
            EstimateDuration::from_secs(100) // Initial 50 + update_period of 50
        );
    }

    #[rstest]
    fn vmclock_time_lag(mut dut_generator: Generator, constant_skew: FullModel) {
        let event = dut_generator.generate(&constant_skew);

        if let EventKind::VMClock(vmclock) = event.variants {
            // Given our fixture's vmclock_time_lag of 1 second
            let event_time = vmclock.vmclock_time;
            let true_time = constant_skew.oscillator().start_time()
                + constant_skew
                    .oscillator_estimate_to_true_duration(dut_generator.props.update_period)
                    .expect("Should convert duration");

            // The vmclock time should be approximately 1 second behind due to the lag
            let difference = true_time - event_time.assume_true();
            assert!(difference.as_seconds_f64() > 0.9 && difference.as_seconds_f64() < 1.1);
        } else {
            panic!("Expected VMClock event");
        }
    }

    #[rstest]
    fn multiple_generations(mut dut_generator: Generator, constant_skew: FullModel) {
        // Generate several events and verify consistency
        let mut last_event = None;
        for i in 0..3 {
            let event = dut_generator.generate(&constant_skew);

            let EventKind::VMClock(vmclock) = event.variants else {
                panic!("Expected VMClock event");
            };
            assert_eq!(vmclock.source_id, "test");
            assert!(vmclock.tsc_timestamp > TscCount::new(0));

            // Verify increasing timestamps
            if i == 0 {
                last_event = Some(vmclock.vmclock_time);
            } else {
                assert!(vmclock.vmclock_time > last_event.unwrap());
                last_event = Some(vmclock.vmclock_time);
            }
        }
    }
}
