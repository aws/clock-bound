//! PHC Time synchronization events

use crate::daemon::time::{Duration, Instant, TscCount};

use super::TscRtt;

/// Contains the PHC and time stamp counter samples to be used by the synchronization algorithm
///
/// `tsc_post` must be greater than `tsc_pre`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Phc {
    /// TSC value before reading reference clock
    tsc_pre: TscCount,
    /// TSC value after reading reference clock
    tsc_post: TscCount,
    /// PHC reference clock info
    data: PhcData,
    #[cfg(all(not(test), feature = "test-side-by-side"))]
    #[serde(skip)]
    system_clock: Option<super::SystemClockMeasurement>,
}

#[bon::bon]
impl Phc {
    /// Construct a [`Phc`]
    ///
    /// Returns `None` if `tsc_post <= tsc_pre`
    #[builder]
    pub fn new(
        tsc_pre: TscCount,
        tsc_post: TscCount,
        data: PhcData,
        #[cfg(all(not(test), feature = "test-side-by-side"))] system_clock: Option<
            super::SystemClockMeasurement,
        >,
    ) -> Option<Self> {
        if tsc_post <= tsc_pre {
            return None;
        }

        Some(Self {
            tsc_pre,
            tsc_post,
            data,
            #[cfg(all(not(test), feature = "test-side-by-side"))]
            system_clock,
        })
    }
}

impl Phc {
    /// `tsc_pre` getter
    pub fn tsc_pre(&self) -> TscCount {
        self.tsc_pre
    }

    /// `tsc_post` getter
    pub fn tsc_post(&self) -> TscCount {
        self.tsc_post
    }

    /// phc data getter
    pub fn data(&self) -> &PhcData {
        &self.data
    }

    /// system time getter
    #[cfg(all(not(test), feature = "test-side-by-side"))]
    pub fn system_clock(&self) -> Option<&super::SystemClockMeasurement> {
        self.system_clock.as_ref()
    }
}

impl TscRtt for Phc {
    fn tsc_pre(&self) -> TscCount {
        self.tsc_pre
    }

    fn tsc_post(&self) -> TscCount {
        self.tsc_post
    }
}

/// PHC specific data
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PhcData {
    /// Reference clock time
    pub time: Instant,
    /// Clock error bound of this measurement
    pub clock_error_bound: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phc_invalid() {
        let result = Phc::builder()
            .tsc_post(TscCount::new(5))
            .tsc_pre(TscCount::new(10)) // pre after post
            .data(PhcData {
                time: Instant::from_days(5),
                clock_error_bound: Duration::from_micros(15),
            })
            .build();

        assert!(result.is_none());
    }

    #[test]
    fn phc_valid() {
        let _result = Phc::builder()
            .tsc_post(TscCount::new(10))
            .tsc_pre(TscCount::new(5)) // pre before post
            .data(PhcData {
                time: Instant::from_days(5),
                clock_error_bound: Duration::from_micros(15),
            })
            .build()
            .unwrap();
    }
}
