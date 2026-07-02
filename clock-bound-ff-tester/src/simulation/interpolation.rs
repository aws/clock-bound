//! simple interpolation logic
//!
//! If you zoom in far enough *every curve looks linear.

use std::ops::{Add, Sub};

use crate::time::{Frequency, Series, TrueDuration};
use num_traits::AsPrimitive;

use crate::simulation::oscillator::FrequencyModel;

/// linear interpolation function
///
/// y - y0   y1 - y0
/// ------ = -------
/// x - x0   x1 - x0
///
/// Therefore:
///
/// y = y0 + (x - x0) * (y1 - y0) / (x1 - x0)
///
/// # NOTE ABOUT PRECISION
/// Floating points are less likely to overflow, but have less precision than using the fixed-point clock-bound duration types.
/// This function compensates by ensuring the differences are done as integer types (this reduces the magnitude of the integer,
/// which minimizes possible precision loss when rounding into floats).
///
/// The downside is this function has no "pure integer" way of doing things. But IMO, that kind of goes out the window
/// once you include arbitrary division.
#[bon::builder] // probably has a performance hit, but this helps correctness
pub fn linear<T, U>(x: T, x0: T, x1: T, y0: U, y1: U) -> U
where
    T: Sub<Output = T> + Copy + AsPrimitive<f64>,
    U: Sub<Output = U> + Add<Output = U> + Copy + AsPrimitive<f64>,
    f64: num_traits::AsPrimitive<U>,
{
    let delta_y = (x - x0).as_() * (y1 - y0).as_() / (x1 - x0).as_();
    y0 + delta_y.as_()
}

#[bon::builder]
pub fn linear_rounded<T, U>(x: T, x0: T, x1: T, y0: U, y1: U) -> U
where
    T: Sub<Output = T> + Copy + AsPrimitive<f64>,
    U: Sub<Output = U> + Add<Output = U> + Copy + AsPrimitive<f64>,
    f64: num_traits::AsPrimitive<U>,
{
    let delta_y = (x - x0).as_() * (y1 - y0).as_() / (x1 - x0).as_();
    let delta_y = delta_y.round();
    y0 + delta_y.as_()
}

/// Interpolate based off of a time series of data
pub trait SeriesInterpolation {
    /// The X axis data type. Usually a time type
    type X;

    /// The Y axis data type, representing the actual data of the time series
    type Y;

    /// Approximate a time value of a series by using linear interpolation
    fn approximate(&self, x: Self::X) -> Option<Self::Y>;

    /// Approximate the time that a data point occurred at
    ///
    /// # NOTE
    /// While X is normally monotonically increasing, there is usually less of a requirement
    /// that the Y axis is the same. If the Y axis is NOT monotonically increasing, this will
    /// return the FIRST valid interpolation value found.
    fn reverse_approximate(&self, y: Self::Y) -> Option<Self::X>;
}

impl SeriesInterpolation for FrequencyModel {
    type X = TrueDuration;
    type Y = Frequency;

    fn approximate(&self, x: Self::X) -> Option<Self::Y> {
        let inner = &self.inner;
        // easy short circuits
        let first = inner.indices().first()?;
        if x < *first {
            return None;
        }

        let last = inner.indices().last()?;
        if x > *last {
            return None;
        }

        let (idx, x_window) = inner
            .indices()
            .windows(2)
            .enumerate()
            .find(|(_, w)| w[1] >= x)?;

        let y_window = &inner.data()[idx..idx + 2];

        // short circuit
        if x_window[0] == x {
            return Some(y_window[0]);
        }
        if x_window[1] == x {
            return Some(y_window[1]);
        }

        let res = linear()
            .x(x.as_nanos())
            .x0(x_window[0].as_nanos())
            .x1(x_window[1].as_nanos())
            .y0(y_window[0].get())
            .y1(y_window[1].get())
            .call();

        Some(Frequency::from_hz(res))
    }

    /// This function is NOT reliable for frequency
    fn reverse_approximate(&self, _y: Self::Y) -> Option<Self::X> {
        panic!("reverse approximate on frequency is UNRELIABLE at best, WRONG at worst")
    }
}

impl<X, Y> SeriesInterpolation for Series<X, Y>
where
    X: PartialOrd + Copy + Into<i64> + From<i64>,
    Y: PartialOrd + Copy + Into<i64> + From<i64>,
{
    type X = X;
    type Y = Y;

    /// A linear interpolation for for [`Series`]
    ///
    /// Given a [`Series`], aka a time series of data. Get they approximate
    /// y value at a certain x point. It does this by searching along the
    /// Series for a window where `x[i] <= x <[i + 1]`, and then using a linear
    /// interpolation to find the corresponding `y` value.
    ///
    /// Below is the best visual representation with my ability to represent diagonal lines
    /// in ASCII
    ///
    /// ```text
    ///      │   Y axis of series                                               
    ///      │                                                     
    ///      │                                                  
    ///      │                                                  
    ///      │                                 o                 
    /// y1 ──┼────────────────────────── o                 
    ///      |                        .  |
    ///      │                      .    │  
    ///     y│◄────────────────── *      |
    ///      |                  . |      |
    ///      |                .   |      |
    /// y0 ──┼──────────────o     |      |                  
    ///      │              │     |      │                  
    ///      │              │     |      │                   
    ///      │   o          │     |      │                
    ///      │              │     |      │                
    ///      │              │     |      │                
    ///      │              │     |      │      
    ///      └──────────────┼────────────┼──────── X axis of series      
    ///                     │     |      │                 
    ///                     │     |      │                 
    ///                                                         
    ///                     x0    x     x1                
    ///
    /// o = Series datapoint; * = interpolated datapoint
    /// ```
    fn approximate(&self, x: X) -> Option<Y> {
        // easy short circuits
        let first = self.indices().first()?;
        if x < *first {
            return None;
        }

        let last = self.indices().last()?;
        if x > *last {
            return None;
        }

        let (idx, x_window) = self
            .indices()
            .windows(2)
            .enumerate()
            .find(|(_, w)| w[1] >= x)?;

        let y_window = &self.data()[idx..idx + 2];

        // short circuit
        if x_window[0] == x {
            return Some(y_window[0]);
        }

        if x_window[1] == x {
            return Some(y_window[1]);
        }

        let res = linear_rounded()
            .x(x.into())
            .x0(x_window[0].into())
            .x1(x_window[1].into())
            .y0(y_window[0].into())
            .y1(y_window[1].into())
            .call();

        Some(Self::Y::from(res))
    }

    fn reverse_approximate(&self, y: Self::Y) -> Option<Self::X> {
        let (idx, y_window) = self
            .data()
            .windows(2)
            .enumerate()
            .find(|(_, w)| (w[0]..=w[1]).contains(&y))?;

        let x_window = &self.indices()[idx..idx + 2];

        // short circuit
        if y_window[0] == y {
            return Some(x_window[0]);
        }

        if y_window[1] == y {
            return Some(x_window[1]);
        }

        // we are finding x from a y value, hence the reversed values
        let res = linear_rounded()
            .x(y.into())
            .x0(y_window[0].into())
            .x1(y_window[1].into())
            .y0(x_window[0].into())
            .y1(x_window[1].into())
            .call();

        Some(Self::X::from(res))
    }
}

#[cfg(test)]
mod test {
    use crate::time::TrueDuration;
    use rstest::rstest;

    use super::*;

    #[test]
    fn linear_interpolation_basic() {
        // Test with simple linear case
        let result = linear().x(5.0f64).x0(0.0).x1(10.0).y0(0.0).y1(10.0).call();
        approx::assert_abs_diff_eq!(result, 5.0);
    }

    #[test]
    fn linear_interpolation_midpoint() {
        // Test exact middle point
        let result = linear().x(5.0f64).x0(0.0).x1(10.0).y0(20.0).y1(30.0).call();
        approx::assert_abs_diff_eq!(result, 25.0);
    }

    #[test]
    fn linear_interpolation_negative_values() {
        // Test with negative values
        let result = linear()
            .x(-5.0f64)
            .x0(-10.0)
            .x1(0.0)
            .y0(-100.0)
            .y1(100.0)
            .call();
        approx::assert_abs_diff_eq!(result, 0.0);
    }

    #[test]
    fn linear_interpolation_exact_bounds() {
        // Test at exact boundary points
        let x1 = linear().x(10.0f64).x0(0.0).x1(10.0).y0(5.0).y1(15.0).call();
        let x0 = linear().x(0.0f64).x0(0.0).x1(10.0).y0(5.0).y1(15.0).call();

        approx::assert_abs_diff_eq!(x1, 15.0);
        approx::assert_abs_diff_eq!(x0, 5.0);
    }

    #[test]
    fn linear_interpolation_outside_bounds() {
        // Test extrapolation outside the bounds
        let result = linear().x(15.0f64).x0(0.0).x1(10.0).y0(0.0).y1(10.0).call();
        approx::assert_abs_diff_eq!(result, 15.0);
    }

    #[test]
    fn linear_interpolation_floating_point_precision() {
        // Test with very small numbers to check floating point precision
        let result = linear().x(0.5f64).x0(0.0).x1(1.0).y0(0.0).y1(1.0).call();
        approx::assert_abs_diff_eq!(result, 0.5);
    }

    #[test]
    fn linear_interpolation_floating_point_y_equals() {
        let result: f64 = linear().x(0.5f64).x0(0.0).x1(1.0).y0(1.0).y1(1.0).call();
        assert!((result - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn linear_interpolation_floating_point_x_equals() {
        let result: f64 = linear().x(0f64).x0(0.0).x1(0.0).y0(0.0).y1(1.0).call();
        assert!(result.is_nan());
    }

    #[rstest]
    #[case(5, 50)]
    #[case(8, 80)]
    fn interpolation_basic(#[case] x: i64, #[case] y: i64) {
        let index = vec![TrueDuration::from_nanos(0), TrueDuration::from_nanos(10)];
        let data = vec![TrueDuration::from_nanos(0), TrueDuration::from_nanos(100)];
        let series = Series::new(index, data).unwrap();

        let result = series.approximate(TrueDuration::from_nanos(x));
        let reverse_result = series.reverse_approximate(TrueDuration::from_nanos(y));
        assert_eq!(result, Some(TrueDuration::from_nanos(y)));
        assert_eq!(reverse_result, Some(TrueDuration::from_nanos(x)));
    }

    #[rstest]
    #[case(0, 5)]
    #[case(10, 25)]
    fn series_interpolation_exact_points(#[case] x: i64, #[case] y: i64) {
        let index = vec![TrueDuration::from_nanos(0), TrueDuration::from_nanos(10)];
        let data = vec![TrueDuration::from_nanos(5), TrueDuration::from_nanos(25)];
        let series = Series::new(index, data).unwrap();

        // Test exact points
        assert_eq!(
            series.approximate(TrueDuration::from_nanos(x)),
            Some(TrueDuration::from_nanos(y))
        );

        assert_eq!(
            series.reverse_approximate(TrueDuration::from_nanos(y)),
            Some(TrueDuration::from_nanos(x))
        );
    }

    #[rstest]
    #[case(-5)]
    #[case(115)]
    fn series_interpolation_out_of_bounds(#[case] x: i64) {
        let index = vec![TrueDuration::from_nanos(0), TrueDuration::from_nanos(10)];
        let data = vec![TrueDuration::from_nanos(0), TrueDuration::from_nanos(100)];
        let series = Series::new(index, data).unwrap();

        // Test point before the series
        assert_eq!(series.approximate(TrueDuration::from_nanos(x)), None);
        assert_eq!(
            series.reverse_approximate(TrueDuration::from_nanos(x)),
            None
        );
    }

    #[rstest]
    #[case(5, 50)]
    #[case(15, 75)]
    fn series_interpolation_multiple_points(#[case] x: i64, #[case] expected_y: i64) {
        let index = vec![
            TrueDuration::from_nanos(0),
            TrueDuration::from_nanos(10),
            TrueDuration::from_nanos(20),
        ];
        let data = vec![
            TrueDuration::from_nanos(0),
            TrueDuration::from_nanos(100),
            TrueDuration::from_nanos(50),
        ];
        let series = Series::new(index, data).unwrap();

        assert_eq!(
            series.approximate(TrueDuration::from_nanos(x)),
            Some(TrueDuration::from_nanos(expected_y))
        );
    }

    #[test]
    fn frequency_model_interpolation() {
        let times = vec![
            TrueDuration::from_nanos(0),
            TrueDuration::from_nanos(100),
            TrueDuration::from_nanos(200),
        ];
        let freqs = vec![
            Frequency::from_hz(1.0),
            Frequency::from_hz(2.0),
            Frequency::from_hz(3.0),
        ];
        let series = Series::new(times.clone(), freqs.clone()).unwrap();
        let model = FrequencyModel { inner: series };

        // Test middle point interpolation
        let mid_point = TrueDuration::from_nanos(50);
        let result = model.approximate(mid_point).unwrap();
        approx::assert_abs_diff_eq!(result.get(), 1.5);

        // Test exact points
        approx::assert_abs_diff_eq!(
            model
                .approximate(TrueDuration::from_nanos(0))
                .unwrap()
                .get(),
            1.0
        );
        approx::assert_abs_diff_eq!(
            model
                .approximate(TrueDuration::from_nanos(100))
                .unwrap()
                .get(),
            2.0
        );

        // Test out of bounds
        assert_eq!(model.approximate(TrueDuration::from_nanos(-50)), None);
        assert_eq!(model.approximate(TrueDuration::from_nanos(300)), None);
    }
}
