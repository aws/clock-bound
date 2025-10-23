//! Code specific to the Truncated distributions.

use rand::{Rng, distributions::Distribution as RandDistribution};

/// Our internal truncated distribution
///
/// Uses rejection sampling to sample from the inner distribution
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Truncated<T> {
    minimum: f64,
    maximum: f64,
    inner: T,
}

impl<T> Truncated<T> {
    /// Returns a Truncated Distribution we can sample from
    ///
    /// # Panics
    /// This function panics if `maximum <= minimum`
    pub fn new(minimum: f64, maximum: f64, inner: T) -> Self {
        assert!(
            (maximum > minimum),
            "Maximum value ({maximum}) must be larger than minimum ({minimum}) value"
        );

        Self {
            minimum,
            maximum,
            inner,
        }
    }
}

impl<T: RandDistribution<f64>> Truncated<T> {
    /// Returns a random value from the truncated  distribution.
    ///
    /// # Panics
    /// This function panics if the distribution is re-sampled equal to or more than 1 million
    /// times. This is an arbitrary number, but it is a reasonable upper bound for the number of
    /// tries needed to sample from the distribution. If this happens, the RNG is likely to be
    /// broken.
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        let mut rv = self.inner.sample(rng);
        let mut loop_count = 0;

        while rv < self.minimum || rv > self.maximum {
            rv = self.inner.sample(rng);

            assert!(
                loop_count < 1_000_000,
                "RNG failed to produce within bounds after enough tries"
            );
            loop_count += 1;
        }
        rv
    }
}

impl<T: RandDistribution<f64>> RandDistribution<f64> for Truncated<T> {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        self.sample(rng)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use crate::simulation::stats::NormalDistribution;
    use rand_chacha;
    use rand_chacha::rand_core::SeedableRng;
    use rstest::rstest;

    #[rstest]
    #[case::succcess(1.0, 2.0, -1.0, 3.0)]
    #[case::succcess(0.0, 1.0, -0.5, 1.5)]
    #[case::succcess(-1.0, 1.0, -1.0, 1.0)]
    pub fn test_truncnormal_distribution_min_max(
        #[case] mean: f64,
        #[case] standard_deviation: f64,
        #[case] min: f64,
        #[case] max: f64,
    ) {
        let normal = NormalDistribution::new(mean, standard_deviation).unwrap();
        let distribution = Truncated::new(min, max, normal);

        let mut rng = rand_chacha::ChaCha12Rng::from_seed(Default::default());

        let mut sample_min = f64::MAX;
        let mut sample_max = f64::MIN;
        for _ in 0..1000 {
            let value = distribution.sample(&mut rng);
            if value < min {
                sample_min = value;
            }
            if value > max {
                sample_max = value;
            }
        }

        assert!(sample_min > min, "sample_min: {sample_min}, min: {min}");

        assert!(sample_max < max, "sample_max: {sample_max}, max: {max}");
    }
}
