//! Code specific to the Normal, Gaussian, statistical distribution.

use rand::{Rng, distributions::Distribution as RandDistribution};
use statrs::distribution::{Normal, NormalError};
use statrs::statistics::Distribution as StatrsDistribution;

/// Our internal Normal distribution wrapper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalDistribution {
    distribution: Normal,
}

impl NormalDistribution {
    /// Returns a Normal distribution struct which we can sample from.
    ///
    /// # Errors
    /// An error will be returned if the standard deviation is 0 or if the backend distribution
    /// library [Normal] distribution function mean and std parameters don't align with what we pass in.
    pub fn new(mean: f64, standard_deviation: f64) -> Result<Self, NormalError> {
        Ok(NormalDistribution {
            distribution: Normal::new(mean, standard_deviation)?,
        })
    }

    /// Returns the mean of the distribution.
    #[expect(
        clippy::missing_panics_doc,
        reason = "normal distribution always has a mean"
    )]
    pub fn mean(&self) -> f64 {
        self.distribution.mean().unwrap()
    }

    /// Returns the standard deviation of the distribution.
    #[expect(
        clippy::missing_panics_doc,
        reason = "normal distribution always has a standard deviation"
    )]
    pub fn standard_deviation(&self) -> f64 {
        self.distribution.std_dev().unwrap()
    }

    /// Pulls a samples from the Normal distribution.
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        self.distribution.sample(rng)
    }
}

impl RandDistribution<f64> for NormalDistribution {
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

    use nalgebra::{DVector, Scalar, dvector};
    use num_traits::Float;
    use varpro::prelude::*;
    use varpro::solvers::levmar::{LevMarProblemBuilder, LevMarSolver};

    fn make_bins(n_bins: usize, min: f64, step_size: f64) -> Vec<f64> {
        let mut rv = vec![];

        for i in 0..n_bins {
            #[expect(
                clippy::cast_precision_loss,
                reason = "It isn't expected for this function to be used for more than 2^53 bins. In the event that it is any loss of precision is ok."
            )]
            rv.push(min + i as f64 * step_size);
        }
        rv
    }

    #[rstest]
    #[case::succcess(1.0, 2.0)]
    #[case::succcess(0.0, 1.0)]
    #[case::succcess(-1.0, 1.0)]
    pub fn check_mean_standard_deviation_sampling(
        #[case] mean: f64,
        #[case] standard_deviation: f64,
    ) {
        let Ok(distribution) = NormalDistribution::new(mean, standard_deviation) else {
            if standard_deviation == 0.0 {
                return;
            }
            panic!("Unexpected error condition!");
        };

        let mut data = vec![];
        let mut rng = rand_chacha::ChaCha12Rng::from_seed(Default::default());

        for _ in 0..1000 {
            let value = distribution.sample(&mut rng);
            data.push(value);
        }

        let data_mean = data.clone().mean();
        assert!(
            (data_mean - mean).abs() < mean.abs() + standard_deviation,
            "mean: {mean}, data_mean: {data_mean}, std: {standard_deviation}",
        );
        let data_std = data.clone().std_dev();
        assert!(
            data_std - standard_deviation < standard_deviation,
            "std: {standard_deviation}, data_std: {data_std}",
        );
    }

    #[test]
    #[allow(clippy::assertions_on_result_states)]
    pub fn invalid_args_constructor() {
        // The constructor should fail due to a standard deviation of 0.
        assert_eq!(
            NormalDistribution::new(0.0, 0.0).unwrap_err(),
            NormalError::StandardDeviationInvalid
        );
    }

    // While the computing the mean and std from a record is great it doesn't provide any
    // shape information. To be sure the shapes of the distribution are correct we
    // perform a fit.
    //
    // For the Normal distribution we fit to the log normal. This transforms our exponential to a
    // linear equation making the derivative the fit requires easier. Note, for simplicity we
    // ignore the scaling factor. This will be accounted for but the correlation with the standard
    // deviation can be ignored.
    //
    // ```latex
    // N = e^{  \frac{(-(x-\mu)^2} {2 \sigma^2} }
    // ```
    //
    // becomes
    //
    // ```latex
    // ln(N) = - (x-\mu)^2 / 2 \sigma^2 + c
    // ```
    //
    // We fit for \mu, \sigma and c, where c is some constant that accounting for scaling.
    #[rstest]
    #[case::succcess(0.0, 1.0, -3.0)]
    #[case::succcess(1.0, 1.5, -2.0)]
    #[case::succcess(-1.0, 1.0, -4.0)]
    pub fn fit_validation(
        #[case] mean: f64,
        #[case] standard_deviation: f64,
        #[case] minimum_bin_edge: f64,
    ) {
        fn model<ScalarType: Scalar + Float + std::convert::From<f64>>(
            x: &DVector<ScalarType>,
            mu: ScalarType,
        ) -> DVector<ScalarType> {
            let two: ScalarType = 2.0.into();
            x.map(|x| -x.powi(2) / two + x * mu - mu.powi(2) / two)
        }

        fn ddmu_model<ScalarType: Scalar + Float + std::convert::From<f64>>(
            x: &DVector<ScalarType>,
            mu: ScalarType,
        ) -> DVector<ScalarType> {
            x.map(|x| x - mu)
        }

        let sigma = standard_deviation;
        let mu = mean;
        let normal_distribution = Normal::new(mu, sigma).unwrap();

        let mut rng = rand_chacha::ChaCha12Rng::from_seed(Default::default());

        // NOTE
        // This is an extremely scrappy histogram making tool.
        // I don't like the popular tools provided in crates.
        // They aren't tailored to this kind of analysis and
        // we may want to develop something in the future.
        let bins = make_bins(12, minimum_bin_edge, 0.5);
        let mut histogram = vec![0; bins.len() - 1];
        for _ in 0..1000 {
            let value = normal_distribution.sample(&mut rng);
            for i in 0..bins.len() - 1 {
                if value > bins[i] && value <= bins[i + 1] {
                    histogram[i] += 1;
                    break;
                }
            }
        }

        let x = {
            let mut v = vec![];
            for i in 0..bins.len() - 1 {
                let offset = (bins[i + 1] - bins[i]) / 2.0;
                v.push(bins[i] + offset);
            }
            v
        };

        let y = {
            let mut temp_y = dvector![];
            for it in &histogram {
                let v = f64::from(*it).ln();
                temp_y = temp_y.push(v);
            }
            temp_y
        };
        let model = SeparableModelBuilder::<f64>::new(&["mu"])
            .initial_parameters(vec![1.0])
            .function(&["mu"], model)
            .partial_deriv("mu", ddmu_model)
            .invariant_function(|x| DVector::from_element(x.len(), 1.0))
            .independent_variable(x.into())
            .build()
            .unwrap();

        let problem = LevMarProblemBuilder::new(model)
            .observations(y)
            .build()
            .unwrap();

        let fit_result = LevMarSolver::default()
            .fit(problem)
            .expect("fit must exit successfully");

        let linear_coefficients = fit_result.linear_coefficients().unwrap();
        let nonlinear_parameters = fit_result.nonlinear_parameters();

        let fit_mean = *nonlinear_parameters.get(0).unwrap();
        let fit_std = (*linear_coefficients.get(0).unwrap()).powf(-0.5);
        assert!(
            (fit_mean - mean).abs() < standard_deviation,
            "fit_std: {fit_std} fit_mean: {fit_mean} mean: {mean} std: {standard_deviation}, Data: {:?}",
            (histogram, bins)
        );
        // NOTE
        // The 2.0 was chosen arbitrarily. Given a large sample size and small enough
        // binning the variance of the standard deviation should much less than the standard
        // deviation itself. The idea here is to check that we are in the right ball park and
        // are flagged if there are any larger issues with implementation.
        assert!(
            (fit_std - standard_deviation).abs() < standard_deviation / 2.0,
            "fit_std: {fit_std} fit_mean: {fit_mean} mean: {mean} std: {standard_deviation}, Data: {:?}",
            (histogram, bins)
        );
    }
}
