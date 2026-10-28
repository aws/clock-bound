//! Writer for the `ClockErrorBoundV3` shared memory segment (shm1).
use nix::sys::time::TimeSpec;

use crate::{
    daemon::{clock_parameters::ClockParameters, time::tsc::Skew},
    shm::{
        ClockErrorBound, ClockErrorBoundGeneric, ClockErrorBoundLayoutVersion, ClockStatus,
        ShmWrite,
    },
};

/// Writes the `ClockErrorBoundV3` layout to the shm1 memory segment.
#[derive(bon::Builder)]
pub(crate) struct Shm1 {
    shm_writer: Box<dyn ShmWrite + Send + Sync>,
    clock_disruption_support_enabled: bool,
    max_drift_ppb: u32,
    disruption_marker: u64,
}

impl Shm1 {
    /// Handles `ClockParameters` SHM1 (`ClockErrorBoundV3`)
    pub fn handle_clock_parameters(
        &mut self,
        clock_parameters: &ClockParameters,
        clock_status: ClockStatus,
    ) {
        if let Some(ceb) = self.create_shm_snapshot(clock_parameters, clock_status) {
            self.shm_writer.write(&ceb);
        }
    }

    /// Handle a clock disruption event for the shm1 segment.
    ///
    /// Updates the disruption marker then writes the `ClockParameters` with
    /// `ClockStatus::Disrupted`.
    pub fn handle_disruption(
        &mut self,
        clock_parameters: &ClockParameters,
        new_disruption_marker: u64,
    ) {
        self.disruption_marker = new_disruption_marker;
        if let Some(ceb) = self.create_shm_snapshot(clock_parameters, ClockStatus::Disrupted) {
            self.shm_writer.write(&ceb);
        }
    }

    /// Build the `ClockErrorBoundV3` layout for the shm1 memory segment.
    fn create_shm_snapshot(
        &self,
        clock_parameters: &ClockParameters,
        clock_status: ClockStatus,
    ) -> Option<ClockErrorBound> {
        let bound_nsec = clock_parameters.clock_error_bound.as_nanos();

        // The conversion is infallible since we use i64 nanoseconds internally
        let as_of = TimeSpec::from(clock_parameters.time);
        let void_after = as_of + TimeSpec::new(1000, 0);

        // The layout carries the error of the period estimate as a dimensionless ratio (the
        // relative error, `period_max_error / period`). This is similar to what V2 folds into
        // `max_drift_ppb`: clients grow the bound by `elapsed × (max_drift + period_err)`.
        // The fractional encoding holds values in [0, 1) only; a relative error of 1 or more means
        // the period estimate is garbage (an error as large as the period itself) and is not
        // written.
        let period_err =
            Skew::from_period_and_error(clock_parameters.period, clock_parameters.period_max_error)
                .get();
        if !(0.0..1.0).contains(&period_err) {
            tracing::error!(
                period_err,
                "Relative period error is not in [0, 1). Skipping writing to SHM."
            );
            return None;
        }

        #[allow(clippy::cast_possible_truncation)]
        #[allow(clippy::cast_sign_loss)]
        let ceb = ClockErrorBoundGeneric::builder()
            .as_of_tsc(clock_parameters.tsc_count.get() as u64)
            .as_of(as_of)
            .void_after(void_after)
            .bound_nsec(bound_nsec)
            .period(clock_parameters.period.get())
            .period_err(period_err)
            .disruption_marker(self.disruption_marker)
            .max_drift_ppb(self.max_drift_ppb)
            .clock_status(clock_status)
            .clock_disruption_support_enabled(self.clock_disruption_support_enabled)
            .build(ClockErrorBoundLayoutVersion::V3);
        Some(ceb)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{MockShmWriter, create_test_clock_parameters};
    use super::*;
    use crate::daemon::time::{Duration, Instant, tsc::Period};
    use rstest::rstest;

    #[rstest]
    #[case::synchronized(ClockStatus::Synchronized)]
    #[case::unknown(ClockStatus::Unknown)]
    #[case::free_running(ClockStatus::FreeRunning)]
    #[case::disrupted(ClockStatus::Disrupted)]
    fn create_shm_snapshot_builds_expected_v3(#[case] clock_status: ClockStatus) {
        let as_of = Instant::from_nanos(3_000_000_000);
        let max_drift_ppb = 15_000;
        let disruption_marker = 345;
        let clock_disruption_support_enabled = true;

        let clock_parameters = create_test_clock_parameters()
            .tsc_count(1_000_000)
            .clock_error_bound_nanos(500)
            .time_nanos(3_000_000_000)
            .call();

        let expected_ceb_v3 = ClockErrorBoundGeneric::builder()
            .as_of_tsc(1_000_000)
            .as_of(TimeSpec::try_from(as_of).unwrap())
            .void_after(TimeSpec::try_from(as_of + Duration::from_secs(1000)).unwrap())
            .period(1e-9)
            // The same 1 % as V2's 10_000_000 ppb, as the ratio the writer computes (1e-11 / 1e-9
            // is not exactly 0.01 in floating point).
            .period_err(
                Skew::from_period_and_error(
                    Period::from_seconds(1e-9),
                    Period::from_seconds(1e-11),
                )
                .get(),
            )
            .bound_nsec(500)
            .disruption_marker(disruption_marker)
            .max_drift_ppb(max_drift_ppb)
            .clock_status(clock_status)
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .build(ClockErrorBoundLayoutVersion::V3);

        // The pure snapshot builder never touches the segment, so no write is expected.
        let mut shm_writer = MockShmWriter::new();
        shm_writer.expect_write().times(0);

        let shm1 = Shm1::builder()
            .shm_writer(Box::new(shm_writer))
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(disruption_marker)
            .build();

        let ceb = shm1
            .create_shm_snapshot(&clock_parameters, clock_status)
            .expect("a valid snapshot is produced for an in-range period error");
        assert_eq!(ceb, expected_ceb_v3);
    }

    /// A relative period error of 1 or more cannot be encoded and means the estimate is garbage:
    /// the snapshot is skipped (`None`) so the caller leaves the V3 segment as it was.
    #[test]
    fn create_shm_snapshot_skips_a_relative_period_error_of_one_or_more() {
        let mut clock_parameters = create_test_clock_parameters()
            .tsc_count(1_000_000)
            .clock_error_bound_nanos(500)
            .time_nanos(3_000_000_000)
            .call();
        clock_parameters.period_max_error = Period::from_seconds(1e-9); // equal to the period

        let mut shm_writer = MockShmWriter::new();
        shm_writer.expect_write().times(0);
        let shm1 = Shm1::builder()
            .shm_writer(Box::new(shm_writer))
            .clock_disruption_support_enabled(true)
            .max_drift_ppb(15_000)
            .disruption_marker(0)
            .build();
        assert!(
            shm1.create_shm_snapshot(&clock_parameters, ClockStatus::Synchronized)
                .is_none()
        );
    }

    #[test]
    fn handle_clock_parameters_shm1() {
        let clock_parameters = create_test_clock_parameters()
            .clock_error_bound_nanos(1000)
            .tsc_count(1000)
            .time_nanos(123_000)
            .call();
        let clock_disruption_support_enabled = false;
        let max_drift_ppb = 0;
        let disruption_marker = 0;
        let clock_status = ClockStatus::Synchronized;

        let mut shm_writer = MockShmWriter::new();
        shm_writer
            .expect_write()
            .withf(move |ceb: &ClockErrorBound| {
                ceb.void_after()
                    == ceb.as_of() + TimeSpec::from_duration(std::time::Duration::from_secs(1000))
                    && ceb.bound_nsec() == 1000
                    && ceb.disruption_marker() == disruption_marker
                    && ceb.clock_status() == clock_status
                    && ceb.clock_disruption_support_enabled() == clock_disruption_support_enabled
            })
            .times(1)
            .return_const(());

        let mut shm1 = Shm1::builder()
            .shm_writer(Box::new(shm_writer))
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(disruption_marker)
            .build();
        shm1.handle_clock_parameters(&clock_parameters, clock_status);
    }

    #[test]
    fn handle_disruption() {
        let clock_disruption_support_enabled = false;
        let max_drift_ppb = 0;
        let initial_disruption_marker = 0;
        let final_disruption_marker = 1;
        let clock_parameters = create_test_clock_parameters()
            .clock_error_bound_nanos(1000)
            .tsc_count(1000)
            .time_nanos(123_000)
            .call();

        let mut shm_writer = MockShmWriter::new();
        shm_writer
            .expect_write()
            .withf(move |ceb: &ClockErrorBound| {
                ceb.void_after()
                    == ceb.as_of() + TimeSpec::from_duration(std::time::Duration::from_secs(1000))
                    && ceb.bound_nsec() == 1000
                    && ceb.disruption_marker() == final_disruption_marker
                    && ceb.max_drift_ppb() == max_drift_ppb
                    && ceb.clock_status() == ClockStatus::Disrupted
                    && ceb.clock_disruption_support_enabled() == clock_disruption_support_enabled
            })
            .times(1)
            .return_const(());

        let mut shm1 = Shm1::builder()
            .shm_writer(Box::new(shm_writer))
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(initial_disruption_marker)
            .build();
        assert_eq!(shm1.disruption_marker, initial_disruption_marker);
        shm1.handle_disruption(&clock_parameters, final_disruption_marker);
        assert_eq!(shm1.disruption_marker, final_disruption_marker);
    }
}
