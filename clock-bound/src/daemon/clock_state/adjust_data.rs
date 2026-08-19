//! Accumulation of clock-adjustment datapoints for the periodic
//! `system_clock_summary` log.

/// Accumulates smooth phase corrections over a window.
#[derive(Debug, Default)]
pub(crate) struct ClockAdjustData {
    count: u64,
    min_ns: i64,
    max_ns: i64,
    sum_ns: i64,
}

impl ClockAdjustData {
    /// Record one smooth phase correction (signed nanoseconds).
    pub(crate) fn record(&mut self, correction_ns: i64) {
        if self.count == 0 {
            self.min_ns = correction_ns;
            self.max_ns = correction_ns;
        } else {
            self.min_ns = self.min_ns.min(correction_ns);
            self.max_ns = self.max_ns.max(correction_ns);
        }
        self.sum_ns += correction_ns;
        self.count += 1;
    }

    /// Number of corrections recorded in the window.
    pub(crate) fn count(&self) -> u64 {
        self.count
    }

    /// Smallest (most negative) correction; `0` for an empty window.
    pub(crate) fn min_ns(&self) -> i64 {
        self.min_ns
    }

    /// Largest correction; `0` for an empty window.
    pub(crate) fn max_ns(&self) -> i64 {
        self.max_ns
    }

    /// Mean correction, truncated toward zero; `0` for an empty window.
    pub(crate) fn mean_ns(&self) -> i64 {
        if self.count == 0 {
            return 0;
        }
        #[expect(
            clippy::cast_possible_wrap,
            reason = "correction count never approaches i64::MAX"
        )]
        let mean_ns = self.sum_ns / self.count as i64;
        mean_ns
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        let data = ClockAdjustData::default();
        assert_eq!(data.count(), 0);
        assert_eq!(data.mean_ns(), 0);
    }

    #[test]
    fn tracks_signed_min_max_mean() {
        let mut data = ClockAdjustData::default();
        // Signed values; the numerically smallest is the most negative.
        // mean = (-3200 + -4800 + -4000 + -4400) / 4 = -16400 / 4 = -4100.
        for correction in [-3200, -4800, -4000, -4400] {
            data.record(correction);
        }
        assert_eq!(data.count(), 4);
        assert_eq!(data.min_ns(), -4800);
        assert_eq!(data.max_ns(), -3200);
        assert_eq!(data.mean_ns(), -4100);
    }

    #[test]
    fn mean_truncates_toward_zero() {
        let mut data = ClockAdjustData::default();
        // (-10 + -10 + -3) / 3 = -23 / 3 = -7 (truncated toward zero from -7.67).
        for correction in [-10, -10, -3] {
            data.record(correction);
        }
        assert_eq!(data.mean_ns(), -7);
    }
}
