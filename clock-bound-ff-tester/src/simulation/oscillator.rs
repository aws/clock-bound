//! Code specific to modelling local hardware oscillators
//!
//! "So it just keeps going?? Unable to stop, unable to slow down??
//! That's horrible! That's a f***ing horror show!" - Neil Kohney, The Other End

use std::f64::consts::PI;

use crate::events::v1::{Oscillator as SerdeOscillator, OscillatorOffsets};
use crate::time::{
    AssumeTrue, CbBridge, DemoteToEstimate, EstimateDuration, Frequency, Series, Skew,
    TrueDuration, TrueInstant, TscCount,
};
use rand::prelude::Distribution;
use statrs::distribution::Normal;

use super::interpolation::SeriesInterpolation;

/// The internal oscillator model used when modeling the system's drift from true time
#[derive(Debug, Clone, PartialEq)]
pub struct Oscillator {
    pub inner: SerdeOscillator,
}

impl From<SerdeOscillator> for Oscillator {
    fn from(inner: SerdeOscillator) -> Self {
        Self { inner }
    }
}

impl From<Oscillator> for SerdeOscillator {
    fn from(outer: Oscillator) -> Self {
        outer.inner
    }
}

/// Represents a source of random noise for oscillator models
///
/// This struct holds parameters for generating random noise to add to oscillator
/// offset models, simulating real-world environmental and electrical noise that
/// affects hardware oscillators.
///
/// # Parameters
///
/// * `rng` - The random number generator used for sampling
/// * `mean` - The mean value of the noise distribution (defaults to 0.0)
/// * `std_dev` - The standard deviation of the noise (defaults to 1.0)
/// * `step_size` - The time interval between consecutive noise samples (defaults to 1 second)
///
/// # Usage
///
/// ```
/// use rand_chacha;
/// use rand_chacha::rand_core::SeedableRng;
/// use clock_bound_ff_tester::time::{TrueDuration, TrueInstant, Frequency};
/// use clock_bound_ff_tester::simulation::oscillator::{Oscillator, Noise};
///
/// let rng = Box::new(rand_chacha::ChaCha12Rng::from_seed(Default::default()));
///
/// let noise = Noise::builder()
///     .rng(rng)
///     .mean(TrueDuration::from_secs(0))
///     .std_dev(TrueDuration::from_secs(5))
///     .step_size(TrueDuration::from_millis(100))
///     .build();
///     
/// let oscillator = Oscillator::create_sin()
///     .clock_frequency(Frequency::from_ghz(1.0))
///     .start_time(TrueInstant::from_days(0))
///     .duration(TrueDuration::from_secs(100))
///     .noise(noise)
///     .call();
/// ```
pub struct Noise {
    pub rng: Box<dyn rand::RngCore>,
    pub mean: TrueDuration,
    pub std_dev: TrueDuration,
    pub step_size: TrueDuration,
}

#[bon::bon]
impl Noise {
    #[builder]
    pub fn new(
        rng: Box<dyn rand::RngCore>,
        #[builder(default = TrueDuration::from_secs(0))] mean: TrueDuration,
        std_dev: TrueDuration,
        #[builder(default = TrueDuration::from_secs(1))] step_size: TrueDuration,
    ) -> Self {
        Self {
            rng,
            mean,
            std_dev,
            step_size,
        }
    }

    pub fn rng_mut(&mut self) -> &mut dyn rand::RngCore {
        &mut *self.rng
    }

    pub fn rng(&self) -> &dyn rand::RngCore {
        &*self.rng
    }

    pub fn mean(&self) -> TrueDuration {
        self.mean
    }

    pub fn standard_deviation(&self) -> TrueDuration {
        self.std_dev
    }

    pub fn step_size(&self) -> TrueDuration {
        self.step_size
    }
}

#[bon::bon]
impl Oscillator {
    /// Return the clock frequency from this model.
    pub fn clock_frequency(&self) -> Frequency {
        self.inner.clock_frequency()
    }

    /// Return the start time from this model.
    pub fn start_time(&self) -> TrueInstant {
        self.inner.start_time()
    }

    /// Return the tsc timestamp start from this model
    pub fn tsc_timestamp_start(&self) -> TscCount {
        self.inner.tsc_timestamp_start()
    }

    /// Return the offset data series from this model
    pub fn offset_series(&self) -> &OscillatorOffsets {
        self.inner.oscillator_offsets()
    }

    /// Create an oscillator with constant parameters
    ///
    /// This oscillator model is only 2 points, where the linear interpolation does
    /// the heavy lifting on all points needed in the middle.
    /// # Parameters
    ///
    /// * `clock_frequency` - The nominal frequency of the oscillator
    /// * `start_time` - The true time when the oscillator model begins
    /// * `duration` - Total time span of the oscillator model
    /// * `tsc_timestamp_start` - Initial value of tsc timestamps (defaults to 0)
    /// * `skew` - The constant frequency error of this oscillator (defaults to 0)
    /// * `starting_oscillator_offset` - Initial offset from true time (defaults to 0)
    /// * `noise` - Optional noise model to add random variations to the offset.
    ///   When provided, the resulting oscillator will have multiple points based on
    ///   the noise model's `step_size` instead of just 2 points.
    #[builder]
    #[expect(clippy::cast_precision_loss)]
    #[expect(clippy::cast_possible_truncation)]
    pub fn create_simple(
        clock_frequency: Frequency,
        start_time: TrueInstant,
        duration: TrueDuration,
        #[builder(default = TscCount::new(0))] tsc_timestamp_start: TscCount,
        #[builder(default)] skew: Skew,
        #[builder(default)] starting_oscillator_offset: TrueDuration,
        noise: Option<Noise>,
    ) -> Self {
        let series_start = (TrueDuration::from_secs(0), starting_oscillator_offset);

        let end_offset = skew.get() * duration.as_nanos() as f64;
        let end_offset = starting_oscillator_offset + TrueDuration::from_nanos(end_offset as i64);
        let series_end = (duration, end_offset);

        let mut offsets: Series<TrueDuration, TrueDuration> =
            [series_start, series_end].into_iter().collect();

        if let Some(mut noise) = noise {
            let normal_dist = Normal::new(
                noise.mean.as_nanos() as f64,
                noise.std_dev.as_nanos() as f64,
            )
            .unwrap();
            let noise_points =
                offsets.indices().last().unwrap().as_nanos() / noise.step_size.as_nanos();
            let offsets_with_noise: Series<TrueDuration, TrueDuration> = (0..noise_points)
                .map(|i| {
                    let x = noise.step_size * usize::try_from(i).unwrap();
                    let y = offsets.approximate(x)
                        .expect("Approximation should succeed as noise sampling points are within the original series range");
                    #[allow(clippy::cast_possible_truncation)]
                    let y = TrueDuration::from_nanos(
                        y.as_nanos() + normal_dist.sample(noise.rng_mut()).round() as i64,
                    );
                    (x, y)
                })
                .collect();
            offsets = offsets_with_noise;
        }

        let inner = SerdeOscillator::builder()
            .clock_frequency(clock_frequency)
            .system_start_time(start_time)
            .tsc_timestamp_start(tsc_timestamp_start)
            .oscillator_offsets(OscillatorOffsets::new(offsets))
            .build();
        Oscillator::from(inner)
    }

    /// Creates an oscillator with a sinusoidal offset pattern.
    ///
    /// This method generates an oscillator whose offset from true time follows a sine wave pattern,
    /// which is useful for modeling periodic variations in clock behavior such as temperature-induced
    /// oscillations. Unlike `create_simple` which creates a linear offset model, this creates a
    /// periodically varying offset that better represents real-world oscillator behavior.
    ///
    /// The model samples the sine wave at regular intervals (defined by `sample_period`) over the
    /// specified `duration`. For each sample point, the offset is calculated as:
    ///
    /// `offset(t) = amplitude * sin(2π * t / period)`
    ///
    /// # Parameters
    ///
    /// * `clock_frequency`: The nominal frequency of the oscillator
    /// * `start_time`: The true time when the oscillator model begins
    /// * `duration`: Total time span of the oscillator model
    /// * `tsc_timestamp_start`: Initial value of tsc timestamps (defaults to 0)
    /// * `period`: Time for one complete sine wave cycle (defaults to 5 minutes)
    /// * `amplitude`: Maximum deviation from true time (defaults to 40 microseconds)
    /// * `sample_period`: Time between consecutive samples in the model (defaults to 1 second)
    /// * `noise` - Optional noise model to add random variations to the sinusoidal pattern.
    ///   When provided, the resulting oscillator will have noise superimposed on the sinusoidal pattern.
    ///
    /// ```text
    ///   Offset
    ///      ^
    ///      |                                                 
    ///   +A |      .                       .                <-- Amplitude
    ///      |    .   .                   .   .               
    ///      |  .       .               .       .             
    ///      | .         .             .         .            
    ///    0 |.-----------.-----------.------------> True Time
    ///      |             .         .                        
    ///      |              .       .                         
    ///      |                .   .                           
    ///  -A  |                  '                            <-- Amplitude
    ///      |                                               
    ///      ||<--------------------->|                      
    ///      |         Period                                
    ///      |                                               
    ///      |      |-|                                     
    ///      |     Sample                                    
    ///      |     Period                                    
    /// ```
    #[builder]
    pub fn create_sin(
        clock_frequency: Frequency,
        start_time: TrueInstant,
        duration: TrueDuration,
        #[builder(default = TscCount::new(0))] tsc_timestamp_start: TscCount,
        #[builder(default = TrueDuration::from_minutes(5))] period: TrueDuration,
        #[builder(default = TrueDuration::from_micros(40))] amplitude: TrueDuration,
        #[builder(default = TrueDuration::from_secs(1))] sample_period: TrueDuration,
        noise: Option<Noise>,
    ) -> Self {
        let num_samples = duration.get() / sample_period.get();

        // Generate the time series of offsets over true time
        let mut offsets: Series<TrueDuration, TrueDuration> = (0..num_samples)
            .map(|i| {
                let x = sample_period * usize::try_from(i).unwrap();
                let y = amplitude.as_seconds_f64()
                    * ((2.0 * PI * x.as_seconds_f64()) / period.as_seconds_f64()).sin();
                let y = TrueDuration::from_seconds_f64(y);
                (x, y)
            })
            .collect();

        if let Some(mut noise) = noise {
            #[allow(clippy::cast_precision_loss)]
            let normal_dist = Normal::new(
                noise.mean.as_nanos() as f64,
                noise.std_dev.as_nanos() as f64,
            )
            .unwrap();
            let noise_points =
                offsets.indices().last().unwrap().as_nanos() / noise.step_size.as_nanos();
            let offsets_with_noise: Series<TrueDuration, TrueDuration> = (0..noise_points)
                .map(|i| {
                    let x = noise.step_size * usize::try_from(i).unwrap();
                    let y = offsets.approximate(x)
                        .expect("Approximation should succeed as noise sampling points are within the original series range");
                    #[allow(clippy::cast_possible_truncation)]
                    let y = y + TrueDuration::from_nanos(normal_dist.sample(noise.rng_mut()).round() as i64);
                    (x, y)
                })
                .collect();
            offsets = offsets_with_noise;
        }

        let offsets = OscillatorOffsets::new(offsets);

        let inner = SerdeOscillator::builder()
            .clock_frequency(clock_frequency)
            .system_start_time(start_time)
            .tsc_timestamp_start(tsc_timestamp_start)
            .oscillator_offsets(offsets)
            .build();

        Oscillator::from(inner)
    }

    /// Return the clock frequencies from this model.
    ///
    /// While the underlying oscillator struct stores the nominal frequency of the oscillator, a change
    /// in offsets means that the actual frequency of the oscillator changes. This function calculates
    /// the actual frequency of the oscillator at each offset.
    ///
    /// This function calculates the frequencies using a first order divided difference (forward looking)
    /// See [here](https://en.wikipedia.org/wiki/Numerical_differentiation) for more info
    ///
    /// NOTE: Because this is using a first order numerical differentiation algorithm,
    /// the length of the output frequencies would be one less than that of the oscillator `offset_series`.
    /// In this implementation, we duplicate the last value to keep the lengths the same    
    #[expect(clippy::missing_panics_doc, reason = "lengths checked at Series::new")]
    #[expect(
        clippy::cast_precision_loss,
        reason = "integer values are close to 0. floats are best we got when dividing"
    )]
    pub fn frequencies(&self) -> FrequencyModel {
        // We calculate frequency by doing
        // frequency[i] = expected_clock_frequency * (1 + skew) where
        //           offset[i + 1] - offset[i]
        // skew = -------------------------------
        //        time_step[i + 1] - time_step[i]
        let time_step_windows = self.inner.oscillator_offsets().indices().windows(2);
        let offset_windows = self.inner.oscillator_offsets().offsets().windows(2);

        let mut frequencies: Vec<_> = time_step_windows
            .zip(offset_windows)
            .map(|(time_step_window, offset_window)| {
                let delta_time_step = (time_step_window[1] - time_step_window[0]).as_nanos() as f64;
                let delta_offset = (offset_window[1] - offset_window[0]).as_nanos() as f64;
                let skew = delta_offset / delta_time_step;
                self.inner.clock_frequency() * (1.0 + skew)
            })
            .collect();

        // duplicate last value make lengths consistent
        frequencies.push(*frequencies.last().unwrap());

        let durations = self.inner.oscillator_offsets().as_ref().indices().to_vec();

        // unwrap okay. Lengths will match
        FrequencyModel {
            inner: Series::new(durations, frequencies).unwrap(),
        }
    }

    /// Return the true-time to tsc timestamp dataset
    ///
    /// This representation of the model shows the relationship between the local oscillator driven
    /// tsc timestamps as they compare to the forward progression of true time.
    ///
    /// This representation will be extremely helpful when calculating time events
    ///
    /// Go from
    /// ```text
    /// | Offset                                        
    /// │                                                      
    /// │                                                      
    /// │                                                      
    /// │                                                      
    /// │       ..............                              ...
    /// │    ....            ..                           ...  
    /// │ ....                 ..                       ...    
    /// ┼.──────────────────────..─────────────────────..─────  True Time
    /// │                        ..                   ..       
    /// │                         ...               ..         
    /// │                           ...          ....          
    /// │                             .....  .....             
    /// │                                 ....                 
    /// │                                                                                          
    /// ```
    ///
    /// To
    ///
    /// ```text
    ///                Oscillator Uncorrected ----->     .         /  <- True Time                              
    ///                                                 ..       /                                
    /// │                                               .      /                                  
    /// │ TSC Timestamp                                 .    /                                    
    /// │                                               .  /                                      
    /// │                                              ../                                        
    /// │                                              /                                         
    /// │                                            /.                                          
    /// │                                          /  .                                          
    /// │                                        /   ..                                          
    /// │                                      /     .                                           
    /// │                                    /      .                                            
    /// │                                  /       ..                                            
    /// │                                /        ..                                             
    /// │                              /         ..                                              
    /// │                            /         ..                                                
    /// │                          /         ...                                                 
    /// │                        /        ...                                                    
    /// │                      /    .......                                                      
    /// │                    /.......                                                            
    /// │              ..../..                                                                   
    /// │        ......  /                                                                        
    /// │     ...      /                                                                          
    /// │   ...      /                                                                            
    /// │  ..      /                                                                              
    /// │ ..     /                                                                                
    /// │.     /                                                                                  
    /// │.   /                                                                                    
    /// │. /                                                                                       
    /// │/.                                                      True Time                                  
    /// │────────────────────────────────────────────────────────────────────────────────────────
    /// ```
    #[expect(clippy::missing_panics_doc, reason = "lengths checked at Series::new")]
    pub fn true_time_to_tsc_timestamps(&self) -> TscCountModel {
        let true_time: Vec<_> = self
            .inner
            .oscillator_offsets()
            .as_ref()
            .absolute_time_indexes(self.inner.start_time())
            .collect();

        let tsc_timestamps = self
            .inner
            .oscillator_offsets()
            .as_ref()
            .iter()
            .map(|(time_step, offset)| {
                // true_time = uncorrected_oscillator_time - offset, therefore
                // true_time + offset = uncorrected_oscillator_time
                let uncorrected_time = (*time_step + *offset).demote_to_estimate();

                // tsc_timestamp / nominal_clock_frequency = uncorrected_oscillator_time, therefore
                // tsc_timestamp = uncorrected_oscillator_time * nominal_clock_frequency
                let tsc_duration = uncorrected_time * self.inner.clock_frequency();
                tsc_duration + self.inner.tsc_timestamp_start()
            })
            .collect();

        TscCountModel {
            inner: Series::new(true_time, tsc_timestamps).unwrap(),
        }
    }
}

/// An [`Oscillator`] with all associated initialized
///
/// This type is purely a pre-optimization and a convenience. By calculating this up front,
/// it removes complexity of calculating different values when generating events
#[derive(Debug, Clone)]
pub struct FullModel {
    oscillator: Oscillator,
    frequencies: FrequencyModel,
    true_time_to_tsc_timestamps: TscCountModel,
}

impl FullModel {
    /// Create a new [`FullModel`] from an [`Oscillator`]
    pub fn calculate_from_oscillator(oscillator: Oscillator) -> Self {
        let frequencies = oscillator.frequencies();
        let true_time_to_tsc_timestamps = oscillator.true_time_to_tsc_timestamps();
        Self {
            oscillator,
            frequencies,
            true_time_to_tsc_timestamps,
        }
    }

    /// Get the [`Oscillator`] model
    pub fn oscillator(&self) -> &Oscillator {
        &self.oscillator
    }

    /// Get the [`FrequencyModel`] model
    pub fn frequencies(&self) -> &FrequencyModel {
        &self.frequencies
    }

    /// Get the [`TscCountModel`] model
    pub fn true_time_to_tsc_timestamps(&self) -> &TscCountModel {
        &self.true_time_to_tsc_timestamps
    }

    /// destructure into the `ff-tester::events` oscillator for serialization
    pub fn to_oscillator(self) -> SerdeOscillator {
        self.oscillator.into()
    }

    /// Calculate a duration in the "True Time" domain for an estimated duration in oscillator time domain
    /// - `estimate` is the estimated time after the start of the oscillator model
    pub fn oscillator_estimate_to_true_duration(
        &self,
        estimate: EstimateDuration,
    ) -> Option<TrueDuration> {
        // first, we need the tsc timestamp at the given oscillator estimate duration
        let estimate_tsc_tstamp = estimate * self.oscillator().clock_frequency()
            + self.oscillator().tsc_timestamp_start();
        // then, we convert that into TrueTime instant with reverse linear interpollation
        let true_instant = self
            .oscillator()
            .true_time_to_tsc_timestamps()
            .as_ref()
            .reverse_approximate(estimate_tsc_tstamp)?;

        // then, we get a duration since oscillator start time, which is an instant in TrueTime
        Some(true_instant - self.oscillator().start_time())
    }
}

/// A representation of how a model's frequency changes with true time
#[derive(Debug, Clone, PartialEq)]
pub struct FrequencyModel {
    pub inner: Series<TrueDuration, Frequency>,
}

impl AsRef<Series<TrueDuration, Frequency>> for FrequencyModel {
    fn as_ref(&self) -> &Series<TrueDuration, Frequency> {
        &self.inner
    }
}

/// A representation of how the tsc timestamp values look over "True Time"
#[derive(Debug, Clone, PartialEq)]
pub struct TscCountModel {
    pub inner: Series<TrueInstant, TscCount>,
}

impl TscCountModel {
    /// [`TrueInstant`] values
    pub fn true_instants(&self) -> &[TrueInstant] {
        self.inner.indices()
    }

    /// [`TscCount`] values
    pub fn tsc_timestamps(&self) -> &[TscCount] {
        self.inner.data()
    }

    /// Convert back to an [`Oscillator`]
    ///
    /// This is helpful for the data collector to capture TSC+System Clock values and convert
    /// back into something that is serializable.
    ///
    /// This function performs [`Oscillator::true_time_to_tsc_timestamps`] in reverse.
    ///
    /// While this function COULD estimate the TSC, it will be more consistent to use system values
    ///
    /// ## Caveats
    /// This function is not PERFECTLY reversible. The resulting [`Oscillator`] will always
    /// start with an offset value of zero. This means that if one had an [`Oscillator`], converted
    /// to a [`TscCountModel`], then converted back to an [`Oscillator`], the resulting
    /// [`Oscillator`] would not be identical to the original.
    ///
    /// This behavior is not seen as an issue. These offsets are equivalent
    pub fn convert_to_oscillator(&self, tsc_frequency: Frequency) -> Oscillator {
        let system_start_time = self.inner.indices()[0];
        let tsc_timestamp_start = self.inner.data()[0];
        let clock_frequency = tsc_frequency;
        let oscillator_iter = self.inner.iter().map(|(true_time, tsc_timestamp)| {
            // reverse of `true_time_to_tsc_timestamps` processing
            let tsc_duration = *tsc_timestamp - tsc_timestamp_start;
            let uncorrected_time = tsc_duration / tsc_frequency;
            let scenario_duration = *true_time - system_start_time;
            let offset = uncorrected_time.into_estimate().assume_true() - scenario_duration;
            (scenario_duration, offset)
        });

        let osc = SerdeOscillator {
            clock_frequency,
            system_start_time,
            tsc_timestamp_start,
            oscillator_offsets: OscillatorOffsets::new(oscillator_iter.collect()),
        };

        Oscillator::from(osc)
    }
}

impl AsRef<Series<TrueInstant, TscCount>> for TscCountModel {
    fn as_ref(&self) -> &Series<TrueInstant, TscCount> {
        &self.inner
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use rand::rngs::StdRng;
    use rand_chacha::rand_core::SeedableRng;

    use crate::time::TrueDuration;
    use rstest::{fixture, rstest};

    #[fixture]
    fn perfect_oscillator() -> Oscillator {
        Oscillator::create_simple()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(365 * 50))
            .tsc_timestamp_start(TscCount::new(10000))
            .duration(TrueDuration::from_secs(100))
            .call()
    }

    #[fixture]
    fn constant_skew_oscillator() -> Oscillator {
        Oscillator::create_simple()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(365 * 50))
            .tsc_timestamp_start(TscCount::new(10000))
            .duration(TrueDuration::from_secs(100))
            .skew(Skew::from_ppm(-15.0))
            .call()
    }

    // this is not really sensible. A constant offset IS a perfect oscillator. But lets unit test this anyways
    #[fixture]
    fn constant_offset_oscillator() -> Oscillator {
        Oscillator::create_simple()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(365 * 50))
            .tsc_timestamp_start(TscCount::new(10000))
            .duration(TrueDuration::from_secs(100))
            .starting_oscillator_offset(TrueDuration::from_micros(50))
            .call()
    }

    // this is not really sensible. A constant offset IS a perfect oscillator. But lets unit test this anyways
    #[fixture]
    fn constant_negative_offset_oscillator() -> Oscillator {
        Oscillator::create_simple()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(365 * 50))
            .tsc_timestamp_start(TscCount::new(10000))
            .duration(TrueDuration::from_secs(100))
            .starting_oscillator_offset(TrueDuration::from_micros(-50))
            .call()
    }

    #[rstest]
    #[case::perfect(perfect_oscillator())]
    #[case::constant_offset(constant_offset_oscillator())]
    #[case::constant_negative_offset(constant_negative_offset_oscillator())]
    fn perfect_frequencies(#[case] expected_perfect_oscillator: Oscillator) {
        let frequencies = expected_perfect_oscillator.frequencies();
        let frequencies = frequencies.as_ref().data();
        assert!(
            frequencies
                .iter()
                .all(|f| *f == expected_perfect_oscillator.inner.clock_frequency())
        );
    }

    #[rstest]
    #[case::perfect(perfect_oscillator())]
    #[case::constant_offset(constant_offset_oscillator())]
    #[case::constant_negative_offset(constant_negative_offset_oscillator())]
    fn lengths_match(#[case] expected_perfect_oscillator: Oscillator) {
        let len = expected_perfect_oscillator.offset_series().as_ref().len();
        assert_eq!(
            len,
            expected_perfect_oscillator
                .true_time_to_tsc_timestamps()
                .as_ref()
                .len()
        );
        assert_eq!(
            len,
            expected_perfect_oscillator.frequencies().as_ref().len()
        );
    }

    #[rstest]
    fn constant_skew_frequencies(constant_skew_oscillator: Oscillator) {
        let frequencies = constant_skew_oscillator.frequencies();
        let first = frequencies.as_ref().data()[0];
        for f in frequencies.as_ref().data() {
            assert!(approx::abs_diff_eq!(first.get(), f.get()));
        }
    }

    #[rstest]
    #[case::perfect(perfect_oscillator())]
    #[case::constant_offset(constant_offset_oscillator())]
    #[case::constant_negative_offset(constant_negative_offset_oscillator())]
    #[case::constant_skew(constant_skew_oscillator())]
    fn true_time_to_tsc_timestamps_is_monotonically_increasing(#[case] oscillator: Oscillator) {
        // while this test is a good sanity check of this function, it's not really verifying much.
        //
        // first, this type doesn't actually guard against the counter decreasing (right now). If the offset jumps an extreme amount,
        // it could create a hardware counter decrease. This is nonsensical, but easy to check as we **generate** oscillator models.
        //
        // secondly, this makes more sense to visually inspect at this point in development
        let timestamp_model = oscillator.true_time_to_tsc_timestamps();

        // Check that the number of timestamps matches input series
        assert_eq!(
            timestamp_model.true_instants().len(),
            oscillator.inner.oscillator_offsets().as_ref().len()
        );

        // Verify timestamps are monotonically increasing
        for pair in timestamp_model.true_instants().windows(2) {
            assert!(pair[0] <= pair[1]);
        }
    }

    #[rstest]
    #[case::perfect(perfect_oscillator())]
    #[case::constant_skew(constant_skew_oscillator())]
    fn calculate_true_to_tsc_and_back_again(#[case] oscillator: Oscillator) {
        let timestamp_model = oscillator.true_time_to_tsc_timestamps();

        let converted_oscillator =
            timestamp_model.convert_to_oscillator(oscillator.clock_frequency());

        assert_eq!(oscillator, converted_oscillator);
    }

    #[rstest]
    #[case::perfect(perfect_oscillator(), 10_000_000_000, 10_000_000_000)]
    #[case::skewed(constant_skew_oscillator(), 10_000_000_000, 10_000_150_000)]
    #[case::offset(constant_offset_oscillator(), 10_000_000_000, 9_999_950_000)]
    fn oscillator_estimate_to_true_duration_matches_expected(
        #[case] oscillator: Oscillator,
        #[case] estimate_nanos: i64,
        #[case] expected_nanos: i64,
    ) {
        let model = FullModel::calculate_from_oscillator(oscillator);
        let estimate = EstimateDuration::from_nanos(estimate_nanos);
        let true_duration = model
            .oscillator_estimate_to_true_duration(estimate)
            .unwrap();
        let expected = TrueDuration::from_nanos(expected_nanos);

        assert!(
            approx::abs_diff_eq!(
                true_duration.as_nanos() as i64,
                expected.as_nanos() as i64,
                epsilon = 100
            ),
            "Expected {expected:?}, got {true_duration:?}"
        );
    }

    #[test]
    fn create_sin_basic_instantiation() {
        // Test with default parameters except the required ones
        let osc = Oscillator::create_sin()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(0))
            .duration(TrueDuration::from_secs(100))
            .call();

        // Verify we have the expected number of samples (with default 1s sampling)
        let offsets = osc.offset_series().as_ref();
        assert_eq!(offsets.len(), 100);
    }

    #[test]
    fn create_sin_amplitude() {
        // Test that amplitude parameter affects the maximum deviation
        let amplitude = TrueDuration::from_micros(75);
        let osc = Oscillator::create_sin()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(0))
            .duration(TrueDuration::from_secs(100))
            .amplitude(amplitude)
            .call();

        // Find the maximum absolute offset
        let max_offset = osc
            .offset_series()
            .as_ref()
            .data()
            .iter()
            .map(|offset| offset.as_nanos().abs())
            .max()
            .unwrap();

        assert_eq!(max_offset, amplitude.as_nanos());
    }

    #[test]
    fn create_sin_period() {
        // Test that period parameter is respected
        let period = TrueDuration::from_secs(10);
        let osc = Oscillator::create_sin()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(0))
            .duration(TrueDuration::from_secs(21))
            .period(period)
            .amplitude(TrueDuration::from_micros(100)) // Large amplitude for clear pattern
            .sample_period(TrueDuration::from_millis(100)) // Finer sampling
            .call();

        let offsets = osc.offset_series().as_ref();

        // Find first two negative-to-positive zero crossings
        let mut first_neg_to_pos_crossing = None;
        let mut second_neg_to_pos_crossing = None;

        for i in 1..offsets.len() {
            if offsets.data()[i - 1].as_nanos() < 0 && offsets.data()[i].as_nanos() >= 0 {
                if first_neg_to_pos_crossing.is_none() {
                    first_neg_to_pos_crossing = Some(i);
                } else if second_neg_to_pos_crossing.is_none() {
                    second_neg_to_pos_crossing = Some(i);
                }
            }
        }

        assert!(
            first_neg_to_pos_crossing.is_some() && second_neg_to_pos_crossing.is_some(),
            "Failed to find two complete cycles"
        );

        // Measure time between crossings (may end up needing to approximate the period)
        let observed_period = offsets.indices()[second_neg_to_pos_crossing.unwrap()]
            - offsets.indices()[first_neg_to_pos_crossing.unwrap()];

        assert_eq!(observed_period.as_nanos(), period.as_nanos());
    }

    #[test]
    fn test_create_sin_sample_period() {
        // Test that sample_period controls the number of points
        let duration = TrueDuration::from_secs(100);

        let osc_1s = Oscillator::create_sin()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(0))
            .duration(duration)
            .sample_period(TrueDuration::from_secs(1))
            .call();

        let osc_2s = Oscillator::create_sin()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(0))
            .duration(duration)
            .sample_period(TrueDuration::from_secs(2))
            .call();

        assert_eq!(osc_1s.offset_series().as_ref().len(), 100); // 100 samples with 1s period
        assert_eq!(osc_2s.offset_series().as_ref().len(), 50); // 50 samples with 2s period
    }

    #[test]
    fn test_create_sin_tsc_timestamp_start() {
        // Test that tsc_timestamp_start is correctly set
        let tsc_start = TscCount::new(12345);
        let osc = Oscillator::create_sin()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(0))
            .duration(TrueDuration::from_secs(10))
            .tsc_timestamp_start(tsc_start)
            .call();

        assert_eq!(osc.tsc_timestamp_start(), tsc_start);
    }

    #[test]
    fn test_create_sin_frequencies() {
        // Test that frequencies model shows variation due to sine wave
        let amplitude = TrueDuration::from_micros(40);
        let period = TrueDuration::from_secs(10);
        let osc = Oscillator::create_sin()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(0))
            .duration(TrueDuration::from_secs(20))
            .amplitude(amplitude)
            .period(period)
            .call();

        let freqs = osc.frequencies();
        let baseline = osc.clock_frequency().get();

        // Check that frequencies vary both above and below nominal
        let mut found_above = false;
        let mut found_below = false;

        for &freq in freqs.as_ref().data() {
            if freq.get() > baseline {
                found_above = true;
            } else if freq.get() < baseline {
                found_below = true;
            }

            if found_above && found_below {
                break;
            }
        }

        assert!(
            found_above && found_below,
            "Expected frequencies to vary both above and below nominal frequency"
        );
    }

    #[test]
    fn simple_oscillator_with_noise() {
        // Deterministic RNG for testing
        let seed = 42;
        let rng = Box::new(rand::rngs::StdRng::seed_from_u64(seed));

        // Significant standard deviation for clear testing
        let noise = Noise::builder()
            .rng(rng)
            .mean(TrueDuration::from_nanos(0))
            .std_dev(TrueDuration::from_millis(100))
            .step_size(TrueDuration::from_millis(10))
            .build();

        let osc_with_noise = Oscillator::create_simple()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(0))
            .duration(TrueDuration::from_secs(1))
            .noise(noise)
            .call();

        // The default simple oscillator only has 2 points, but with noise it should have more
        let points_count = osc_with_noise.offset_series().as_ref().len();
        assert!(
            points_count > 2,
            "Simple oscillator with noise should have more than the default 2 points"
        );
    }

    #[test]
    fn sine_oscillator_with_noise() {
        // Deterministic RNG for testing
        let seed = 42;
        let rng = Box::new(rand::rngs::StdRng::seed_from_u64(seed));

        // Significant standard deviation for clear testing
        let noise = Noise::builder()
            .rng(rng)
            .mean(TrueDuration::from_nanos(0))
            .std_dev(TrueDuration::from_millis(100))
            .step_size(TrueDuration::from_millis(10))
            .build();

        let osc_with_noise = Oscillator::create_sin()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(0))
            .duration(TrueDuration::from_secs(1))
            .sample_period(TrueDuration::from_millis(10))
            .noise(noise)
            .call();

        // Identical oscillator without noise
        let osc_without_noise = Oscillator::create_sin()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(0))
            .duration(TrueDuration::from_secs(1))
            .sample_period(TrueDuration::from_millis(10))
            .call();

        // Get offsets from both oscillators
        let noisy_offsets = osc_with_noise.offset_series().as_ref().data();
        let clean_offsets = osc_without_noise.offset_series().as_ref().data();

        // Noise is applied onto the offset series which indexed by as many sample periods there
        // are in a given duration. A noisy offset series can be less in size by 1.
        assert_eq!(noisy_offsets.len(), clean_offsets.len() - 1);

        // Verify at least some points differ (noise should have changed them)
        let mut has_difference = false;
        for (noisy, clean) in noisy_offsets.iter().zip(clean_offsets.iter()) {
            if noisy.as_nanos() != clean.as_nanos() {
                has_difference = true;
                break;
            }
        }

        assert!(
            has_difference,
            "Noise should have changed at least some offset values"
        );
    }

    #[test]
    fn noise_statistical_properties() {
        // Test that noise follows expected statistical properties
        let seed = 42;
        let std_dev_ns: f64 = 100.0;
        let mean: f64 = 0.0;

        let rng = Box::new(StdRng::seed_from_u64(seed));

        #[allow(clippy::cast_precision_loss)]
        #[allow(clippy::cast_possible_truncation)]
        let noise = Noise::builder()
            .rng(rng)
            .mean(TrueDuration::from_nanos(mean.round() as i64))
            .std_dev(TrueDuration::from_nanos(std_dev_ns.round() as i64))
            .step_size(TrueDuration::from_millis(10))
            .build();

        // Create a flat baseline oscillator (all zeros) so we can isolate noise
        let osc_with_noise = Oscillator::create_sin()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(0))
            .duration(TrueDuration::from_secs(2))
            .amplitude(TrueDuration::from_nanos(0)) // Zero amplitude = flat line
            .sample_period(TrueDuration::from_millis(10))
            .noise(noise)
            .call();

        let noisy_offsets = osc_with_noise.offset_series().as_ref().data();

        // Calculate mean
        let sum: i64 = noisy_offsets.iter().map(|d| d.as_nanos()).sum();
        #[allow(clippy::cast_precision_loss)]
        let calculated_mean = sum as f64 / noisy_offsets.len() as f64;

        // Calculate standard deviation
        #[allow(clippy::cast_precision_loss)]
        let variance_sum: f64 = noisy_offsets
            .iter()
            .map(|d| {
                let diff = d.as_nanos() as f64 - calculated_mean;
                diff * diff
            })
            .sum();
        #[allow(clippy::cast_precision_loss)]
        let calculated_std_dev = (variance_sum / noisy_offsets.len() as f64).sqrt();

        // We expect the calculated mean to be near the specified mean (allowing some deviation)
        assert!(
            (calculated_mean - mean).abs() < std_dev_ns,
            "Calculated mean {calculated_mean} should be close to specified mean {mean}"
        );

        // For std dev, we expect it to be within a conservative 30% of the specified value
        assert!(
            (calculated_std_dev - std_dev_ns).abs() / std_dev_ns < 0.3,
            "Calculated std dev {calculated_std_dev} should be close to specified std dev {std_dev_ns}"
        );
    }

    #[test]
    fn noise_with_extreme_parameters() {
        // Test with very small standard deviation (close to zero effect)
        let seed = 42;
        let rng = Box::new(StdRng::seed_from_u64(seed));

        let tiny_noise = Noise::builder()
            .rng(rng)
            .mean(TrueDuration::from_nanos(0))
            .std_dev(TrueDuration::from_nanos(1))
            .step_size(TrueDuration::from_millis(10))
            .build();

        let osc_tiny_noise = Oscillator::create_sin()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(0))
            .duration(TrueDuration::from_millis(100))
            .amplitude(TrueDuration::from_micros(10))
            .sample_period(TrueDuration::from_millis(10))
            .noise(tiny_noise)
            .call();

        // Create identical oscillator without noise
        let osc_no_noise = Oscillator::create_sin()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(0))
            .duration(TrueDuration::from_millis(100))
            .amplitude(TrueDuration::from_micros(10))
            .sample_period(TrueDuration::from_millis(10))
            .call();

        // With tiny standard deviation, the outputs should be very close to the no-noise version
        let tiny_noise_offsets = osc_tiny_noise.offset_series().as_ref().data();
        let no_noise_offsets = osc_no_noise.offset_series().as_ref().data();

        for (o1, o2) in tiny_noise_offsets.iter().zip(no_noise_offsets.iter()) {
            approx::assert_abs_diff_eq!(
                o1.as_nanos(),
                o2.as_nanos(),
                epsilon = 2 // Allow 2ns difference due to tiny std dev noise
            );
        }

        // Test with large mean offset (should shift all values)
        let large_mean = 5000; // 5 microseconds
        let rng = Box::new(StdRng::seed_from_u64(seed));
        let mean_shift_noise = Noise::builder()
            .rng(rng)
            .mean(TrueDuration::from_nanos(large_mean))
            .std_dev(TrueDuration::from_nanos(1)) // Minimal std dev to isolate mean effect
            .step_size(TrueDuration::from_millis(10))
            .build();

        let osc_shifted = Oscillator::create_sin()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(0))
            .duration(TrueDuration::from_millis(100))
            .amplitude(TrueDuration::from_micros(0)) // Zero amplitude to isolate noise effect
            .sample_period(TrueDuration::from_millis(10))
            .noise(mean_shift_noise)
            .call();

        let shifted_offsets = osc_shifted.offset_series().as_ref().data();

        // All offsets should be close to the specified mean
        #[allow(clippy::cast_precision_loss)]
        for offset in shifted_offsets {
            assert!(
                (offset.as_nanos() - large_mean).abs() <= 1,
                "Offsets should be shifted by the noise mean value"
            );
        }

        // Test with very large standard deviation
        let rng = Box::new(StdRng::seed_from_u64(seed));
        let large_noise = Noise::builder()
            .rng(rng)
            .mean(TrueDuration::from_nanos(0))
            .std_dev(TrueDuration::from_millis(1)) // 1ms std dev
            .step_size(TrueDuration::from_millis(10))
            .build();

        let osc_large_noise = Oscillator::create_sin()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(0))
            .duration(TrueDuration::from_secs(1))
            .amplitude(TrueDuration::from_micros(10)) // Small sine amplitude
            .sample_period(TrueDuration::from_millis(10))
            .noise(large_noise)
            .call();

        // With large noise, the standard deviation of the result should
        // be much larger than the sine amplitude
        let large_noise_offsets = osc_large_noise.offset_series().as_ref().data();

        // Calculate standard deviation
        #[allow(clippy::cast_precision_loss)]
        let sum: i64 = large_noise_offsets.iter().map(|d| d.as_nanos()).sum();
        #[allow(clippy::cast_precision_loss)]
        let mean_offset = sum as f64 / large_noise_offsets.len() as f64;

        #[allow(clippy::cast_precision_loss)]
        let variance_sum: f64 = large_noise_offsets
            .iter()
            .map(|d| {
                let diff = d.as_nanos() as f64 - mean_offset;
                diff * diff
            })
            .sum();
        #[allow(clippy::cast_precision_loss)]
        let std_dev = (variance_sum / large_noise_offsets.len() as f64).sqrt();

        // The std dev of the output should be much larger than the sine amplitude (10μs)
        assert!(
            std_dev > 50_000.0, // Should be much larger than the 10μs sine amplitude
            "Large noise should dominate the sine wave pattern"
        );
    }
}
