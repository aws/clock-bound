//! Struct for serializing local oscillator models
//!
//! We model oscillators via their offset from true time as it changes over time. The exact mechanism of
//! how these offsets are GENERATED is out of scope of this crate. Instead this crate focuses on the REPRESENTATION
//! of an oscillator model, and how to serialize and deserialize it.

use crate::time::{Frequency, Series, TrueDuration, TrueInstant, TscCount};
use serde::{Deserialize, Serialize};

/// The simulated clock model is a representation of the relationship of True Time to the local hardware oscillator offset (as a True Duration)
///
/// This type enables 2 major aspects.
/// 1. It shows the offset (and skew) of the local hardware oscillator based off true time, and
/// 2. it enables conversions between tsc timestamps and true time
///
/// ## Offsets and skews
/// The skew is the error of the clock frequency from its nominal value. The skew can be influenced by many real world phenomena, like
/// temperature, hardware age, etc.
///
/// The offset is the present time error of a clock from the true value.
///
/// The `time_steps` and `offset` values shows how the offset values change over time.
///
/// The `skew` is the error in the clocks frequency which effectively leads to offsets. The `skew` is the derivative (slope) of this dataset
///
/// ## Conversions between tsc timestamps and true time
///
/// The data serialized enables the creation of other data-sets (without duplicating or otherwise over-constraining the data).
///
/// A [`TscCount`] at a [`TrueInstant`] can be calculated as
/// ```text
/// tsc_timestamp[i] = start_time + ((time_steps[i] - offset[i]) * clock_frequency) + tsc_timestamp_start
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, bon::Builder)]
pub struct Oscillator {
    /// The start time of the clock model
    pub system_start_time: TrueInstant,
    /// The corresponding tsc timestamp to the `start_time`
    ///
    /// When a scenario starts, the first offset should happen at `start_time`,
    /// and that will correspond to the tsc timestamp at `tsc_timestamp_start`
    #[serde(alias = "raw_timestamp_start")]
    pub tsc_timestamp_start: TscCount,
    /// The nominal clock frequency in Hz
    ///
    /// This corresponds to a skew of 0 ppm in this graph.
    pub clock_frequency: Frequency,
    /// Data series of true duration time steps to their offsets
    ///
    /// The index of this data series are the simulated time step offsets.
    /// The data of this data series are the offsets from true time of the local oscillator
    pub oscillator_offsets: OscillatorOffsets,
}

impl Oscillator {
    /// Start Time
    pub fn start_time(&self) -> TrueInstant {
        self.system_start_time
    }

    /// TSC Timestamp Start
    pub fn tsc_timestamp_start(&self) -> TscCount {
        self.tsc_timestamp_start
    }

    /// Clock Frequency
    pub fn clock_frequency(&self) -> Frequency {
        self.clock_frequency
    }

    /// The offset series
    pub fn oscillator_offsets(&self) -> &OscillatorOffsets {
        &self.oscillator_offsets
    }
}

/// A representation of a time series of oscillator offsets
///
/// The X axis of this time series is the true time,
/// The Y axis shows the offset of the oscillator from true time
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OscillatorOffsets {
    pub inner: Series<TrueDuration, TrueDuration>,
}

impl OscillatorOffsets {
    /// Create a new oscillator offset series
    pub fn new(inner: Series<TrueDuration, TrueDuration>) -> Self {
        Self { inner }
    }

    /// Return the time series indices
    pub fn indices(&self) -> &[TrueDuration] {
        self.inner.indices()
    }

    /// Return the offsets of the time series
    pub fn offsets(&self) -> &[TrueDuration] {
        self.inner.data()
    }
}

impl AsRef<Series<TrueDuration, TrueDuration>> for OscillatorOffsets {
    fn as_ref(&self) -> &Series<TrueDuration, TrueDuration> {
        &self.inner
    }
}

impl From<Series<TrueDuration, TrueDuration>> for OscillatorOffsets {
    fn from(inner: Series<TrueDuration, TrueDuration>) -> Self {
        Self { inner }
    }
}
