//! Building blocks for all your delay needs.

use crate::simulation::stats::Distribution;
use crate::time::TrueDuration;
use rand::Rng;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimeUnit {
    Secs,
    Millis,
    Micros,
    Nanos,
}

/// Building block that allows the user to define a network delay using distribution models.
#[derive(Debug, Clone, PartialEq)]
pub struct Delay<T: Distribution> {
    /// The statistical distribution model the delay will be pulled from.
    pub distribution: T,
    /// The units of measurement which to apply to the sampled values.
    pub unit: TimeUnit,
}

impl<T: Distribution> Delay<T> {
    pub fn get_value<R: Rng + ?Sized>(&self, rng: &mut R) -> TrueDuration {
        let rng_value = self.distribution.sample(rng);
        match self.unit {
            TimeUnit::Secs => TrueDuration::from_seconds_f64(rng_value),
            TimeUnit::Millis => TrueDuration::from_millis_f64(rng_value),
            TimeUnit::Micros => TrueDuration::from_micros_f64(rng_value),
            TimeUnit::Nanos => TrueDuration::from_nanos_f64(rng_value),
        }
    }

    pub fn new(distribution: T, unit: TimeUnit) -> Self {
        Self { distribution, unit }
    }
}

/// Convenience trait intended to make `Generator` and cli integration easier.
pub trait DelayRng: std::fmt::Debug {
    fn get_value(&self, rng: &mut dyn rand::RngCore) -> TrueDuration;

    fn get_value_tsc(&self, rng: &mut dyn rand::RngCore) -> (f64, TimeUnit);
}
impl<T: Distribution> DelayRng for Delay<T> {
    fn get_value(&self, rng: &mut dyn rand::RngCore) -> TrueDuration {
        self.get_value(rng)
    }

    fn get_value_tsc(&self, rng: &mut dyn rand::RngCore) -> (f64, TimeUnit) {
        let rng_value = self.distribution.sample(rng);
        (rng_value, self.unit)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::simulation::stats;
    use rand_chacha;
    use rand_chacha::rand_core::SeedableRng;
    use rstest::rstest;

    #[rstest]
    #[case(TimeUnit::Secs)]
    #[case(TimeUnit::Millis)]
    #[case(TimeUnit::Micros)]
    #[case(TimeUnit::Nanos)]
    fn validate_delay_units(#[case] unit: TimeUnit) {
        // expectations
        //
        // This is intended to check the mapping between delay values and the units of time we set.
        // We use the Dirac distribution for this test because there is no variance. Sampling the Dirac
        // distribution always returns the same value.
        let v = 10.0;
        let delay = Delay {
            distribution: stats::DiracDistribution::new(v).unwrap(),
            unit,
        };

        let mut rng = rand_chacha::ChaCha12Rng::from_seed(Default::default());
        let delay_value = delay.get_value(&mut rng);
        #[allow(clippy::cast_possible_truncation)]
        let expected_value = match unit {
            TimeUnit::Secs => TrueDuration::from_secs(v as i64),
            TimeUnit::Millis => TrueDuration::from_millis(v as i64),
            TimeUnit::Micros => TrueDuration::from_micros(v as i64),
            TimeUnit::Nanos => TrueDuration::from_nanos(v as i64),
        };

        assert_eq!(expected_value, delay_value);

        let (_, out_unit) = delay.get_value_tsc(&mut rng);
        assert_eq!(out_unit, unit);
    }
}
