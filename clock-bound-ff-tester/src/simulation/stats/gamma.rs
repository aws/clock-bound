//! Code specific to the Gamma statistical distribution.

use rand::{Rng, distributions::Distribution as RandDistribution};
use statrs::distribution::{Gamma, GammaError};

/// Our internal Gamma distribution wrapper.
///
/// Unlike the `statrs` distribution we include `loc` as a parameter.
/// This parameter allows us to shift the probability distribution.
/// This implementation and naming is pulled from scipy.
/// <https://docs.scipy.org/doc/scipy/reference/generated/scipy.stats.gamma.html>
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(dead_code)]
pub struct GammaDistribution {
    distribution: Gamma,
    loc: f64,
}

impl GammaDistribution {
    /// Returns a Gamma distribution struct which we can sample from.
    ///
    /// # Errors
    /// An error will be returned if the input parameters are ill defined.
    pub fn new(shape: f64, rate: f64, loc: f64) -> Result<Self, GammaError> {
        Ok(GammaDistribution {
            distribution: Gamma::new(shape, rate)?,
            loc,
        })
    }

    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        self.distribution.sample(rng) + self.loc
    }

    pub fn shape(&self) -> f64 {
        self.distribution.shape()
    }

    pub fn rate(&self) -> f64 {
        self.distribution.rate()
    }

    pub fn loc(&self) -> f64 {
        self.loc
    }
}

impl RandDistribution<f64> for GammaDistribution {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        self.sample(rng)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use rand_chacha;
    use rand_chacha::rand_core::SeedableRng;
    use rstest::rstest;
    use statrs::statistics::Statistics;

    #[rstest]
    #[case::standard_case(2.0, 1.0, 0.0)]
    #[case::significant_rate(2.0, 10.0, 0.0)]
    #[case::significant_loc(2.0, 1.0, 5.0)]
    fn check_mean_standard_deviation_sampling(
        #[case] shape: f64,
        #[case] rate: f64,
        #[case] loc: f64,
    ) {
        let Ok(distribution) = GammaDistribution::new(shape, rate, loc) else {
            panic!("Unexpected error condition!");
        };

        let mut data = vec![];
        let mut rng = rand_chacha::ChaCha12Rng::from_seed(Default::default());

        for _ in 0..1000 {
            let value = distribution.sample(&mut rng);
            data.push(value);
        }

        let data_mean = data.clone().mean();
        let data_std = data.clone().std_dev();

        let expected_mean = shape / rate + loc;
        let expected_std = shape.powf(0.5) / rate;

        assert!(
            (data_mean - expected_mean).abs() < expected_mean.abs() + expected_std,
            "expected mean: {expected_mean}, data mean: {data_mean}, std: {data_std}"
        );

        assert!(
            (data_std - expected_std).abs() < expected_std * 0.5,
            "expected std: {expected_std}, data std: {data_std}"
        );
    }

    #[rstest]
    #[case::shape_invalid(-1.0, 1.0, GammaError::ShapeInvalid)]
    #[case::rate_invalid(1.0, -1.0, GammaError::RateInvalid)]
    #[case::shape_rate_infinite(f64::INFINITY, f64::INFINITY, GammaError::ShapeAndRateInfinite)]
    pub fn invalid_args_constructor(
        #[case] shape: f64,
        #[case] rate: f64,
        #[case] expected_error: GammaError,
    ) {
        assert_eq!(
            GammaDistribution::new(shape, rate, 0.0),
            Err(expected_error)
        );
    }
}
