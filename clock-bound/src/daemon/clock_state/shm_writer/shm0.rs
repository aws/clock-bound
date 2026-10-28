//! Writer for the `ClockErrorBoundV2` shared memory segment (shm0).
use nix::sys::time::TimeSpec;
use tracing::info;

use crate::{
    daemon::{
        clock_parameters::ClockParameters,
        io::tsc::ReadTscImpl,
        time::{
            ClockExt, Duration, Instant, TscCount, clocks,
            tsc::{Period, Skew},
        },
    },
    shm::{
        ClockErrorBound, ClockErrorBoundGeneric, ClockErrorBoundLayoutVersion, ClockStatus,
        ShmWrite,
    },
};

/// Writes the `ClockErrorBoundV2` layout to the shm0 memory segment.
#[derive(bon::Builder)]
pub struct Shm0 {
    shm_writer: Box<dyn ShmWrite + Send + Sync>,
    clock_disruption_support_enabled: bool,
    max_drift_ppb: u32,
    disruption_marker: u64,
}

impl Shm0 {
    /// Initialize the `ClockErrorBoundV2` segment on daemon startup.
    ///
    /// ClockBound 2.0 clients rely on the OS system clock to 1) create a timestamp, and 2) grow
    /// the value of the CEB since it was last computed.
    ///
    /// Upon restart, we may be stepping the OS clock. Consequently, we cannot let clients use
    /// data from an existing SHM segment, as this would break all guarantees we must provide.
    /// To be safe, we initialize and write the SHM segment with `ClockStatus::Unknown` BEFORE any
    /// adjustments are done. This will lead to a short period of time during which clients will
    /// see the clock not being useable. We have a fairly aggressive burst mode on start to
    /// minimize this interruption.
    pub fn initialize(&mut self) {
        info!(
            "Initializing SHM segment to status `ClockStatus::Unknown` and zeroing other fields."
        );
        let clock_parameters = ClockParameters {
            tsc_count: TscCount::new(0),
            time: Instant::new(0),
            clock_error_bound: Duration::new(0),
            period: Period::from_seconds(0.0),
            period_max_error: Period::from_seconds(0.0),
            as_of_monotonic: Instant::new(0),
        };
        if let Some(ceb) = self.create_shm_snapshot(&clock_parameters, ClockStatus::Unknown) {
            self.shm_writer.write(&ceb);
        }
    }

    /// Handles `ClockParameters` for SHM0 (`ClockErrorBoundV2`)
    ///
    /// # Panics
    /// Panics if error bound calculated exceeds `i64::MAX`
    pub fn handle_clock_parameters(
        &mut self,
        clock_parameters: &ClockParameters,
        clock_status: ClockStatus,
    ) {
        // The kernel system clock is used on `ClockErrorBoundV2` client reads, so the error between that and
        // the `ClockBound` clock calculated from `ClockParameters` needs to be added in, thus we measure it here
        let clockbound_clock = clocks::ClockBound::new(clock_parameters.clone(), ReadTscImpl);
        let offset_and_rtt = clockbound_clock.get_offset_and_rtt(&clocks::RealTime);

        let clock_error_bound = clock_parameters.clock_error_bound
            + offset_and_rtt.offset().abs()
            + (offset_and_rtt.rtt() / 2);

        let shm0_clock_parameters = ClockParameters {
            clock_error_bound,
            ..clock_parameters.clone()
        };
        if let Some(ceb) = self.create_shm_snapshot(&shm0_clock_parameters, clock_status) {
            self.shm_writer.write(&ceb);
        }
    }

    /// Handle a clock disruption event for the shm0 segment.
    ///
    /// Writes the raw `ClockParameters` straight through with `ClockStatus::Disrupted` (no
    /// offset/rtt measurement), updating the disruption marker first.
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

    /// Build the `ClockErrorBoundV2` layout for the shm0 memory segment.
    //
    // TODO: for this function to work, we require mutating the ClockParameters value with
    // modifications from reading the CLOCK_REALTIME. That is not a clean interface
    fn create_shm_snapshot(
        &self,
        clock_parameters: &ClockParameters,
        clock_status: ClockStatus,
    ) -> Option<ClockErrorBound> {
        // XXX: here as_of is the CLOCK_MONOTONIC_COARSE timestamp taken *before* the clock
        // parameters were computed.
        let as_of = TimeSpec::from(clock_parameters.as_of_monotonic);
        let void_after = as_of + TimeSpec::new(1000, 0);

        let bound_nsec = clock_parameters.clock_error_bound.as_nanos();

        // The shared memory segment layout V2 (for ClockBound 2.0 clients) does not have a field
        // to distinguish between the growth of the CEB due to the hardware worse case, from the
        // growth due to the fact our period estimate is ... an estimate, and carry some residual
        // skew. Consequently, we squash the two notions into one, so that the clients can grow the
        // CEB correctly, accounting for the duration between `as_of` and the instant they read the
        // clock.
        let software_skew =
            Skew::from_period_and_error(clock_parameters.period, clock_parameters.period_max_error);
        let Some(software_skew_ppb) = software_skew.to_ppb() else {
            tracing::error!(
                "Software skew is too large to be expressed as a ppb. Skipping writing to SHM."
            );
            return None;
        };
        let max_drift_ppb = self.max_drift_ppb + software_skew_ppb;

        // Build the ClockErrorBound::V2 layout.
        let ceb = ClockErrorBoundGeneric::builder()
            .as_of(as_of)
            .void_after(void_after)
            .bound_nsec(bound_nsec)
            .disruption_marker(self.disruption_marker)
            .max_drift_ppb(max_drift_ppb)
            .clock_status(clock_status)
            .clock_disruption_support_enabled(self.clock_disruption_support_enabled)
            .build(ClockErrorBoundLayoutVersion::V2);

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
    fn create_shm_snapshot_builds_expected_v2(#[case] clock_status: ClockStatus) {
        let as_of = Instant::from_nanos(3_000_000_000);
        let bound_nsec = 500;
        let max_drift_ppb = 15_000;
        let disruption_marker = 345;
        let clock_disruption_support_enabled = true;
        let expected_ceb_v2 = ClockErrorBoundGeneric::builder()
            .as_of(TimeSpec::try_from(as_of).unwrap())
            .void_after(TimeSpec::try_from(as_of + Duration::from_secs(1000)).unwrap())
            .bound_nsec(bound_nsec)
            .disruption_marker(disruption_marker)
            .max_drift_ppb(max_drift_ppb + 10000000) // Clock params have 1% period error, in PPB
            .clock_status(clock_status)
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .build(ClockErrorBoundLayoutVersion::V2);

        let clock_parameters = create_test_clock_parameters()
            .tsc_count(1_000_000)
            .clock_error_bound_nanos(500)
            .time_nanos(3_000_000_000)
            .call();

        // The pure snapshot builder never touches the segment, so no write is expected.
        let mut shm_writer = MockShmWriter::new();
        shm_writer.expect_write().times(0);

        let shm0 = Shm0::builder()
            .shm_writer(Box::new(shm_writer))
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(disruption_marker)
            .build();

        let ceb = shm0
            .create_shm_snapshot(&clock_parameters, clock_status)
            .expect("a valid snapshot is produced for in-range skew");
        assert_eq!(ceb, expected_ceb_v2);
    }

    /// When the software skew cannot be expressed as a ppb the snapshot is skipped (`None`) so the
    /// caller leaves the segment unchanged.
    #[test]
    fn create_shm_snapshot_skips_when_skew_too_large() {
        let mut clock_parameters = create_test_clock_parameters()
            .tsc_count(1_000_000)
            .clock_error_bound_nanos(500)
            .time_nanos(3_000_000_000)
            .call();
        // A period error far larger than the period produces a skew that cannot fit in a ppb.
        clock_parameters.period_max_error = Period::from_seconds(1e9);

        let mut shm_writer = MockShmWriter::new();
        shm_writer.expect_write().times(0);

        let shm0 = Shm0::builder()
            .shm_writer(Box::new(shm_writer))
            .clock_disruption_support_enabled(true)
            .max_drift_ppb(15_000)
            .disruption_marker(0)
            .build();

        assert!(
            shm0.create_shm_snapshot(&clock_parameters, ClockStatus::Synchronized)
                .is_none()
        );
    }

    #[test]
    fn handle_clock_parameters_shm0() {
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
                    && ceb.disruption_marker() == disruption_marker
                    && ceb.clock_status() == clock_status
                    && ceb.clock_disruption_support_enabled() == clock_disruption_support_enabled
            })
            .times(1)
            .return_const(());

        let mut shm0 = Shm0::builder()
            .shm_writer(Box::new(shm_writer))
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(disruption_marker)
            .build();
        shm0.handle_clock_parameters(&clock_parameters, clock_status);
    }

    #[test]
    fn test_initialize_ceb_v2_shm() {
        let clock_disruption_support_enabled = false;
        let expected_max_drift_ppb = 15_000;
        let expected_disruption_marker = 0;
        let mut shm_writer = MockShmWriter::new();
        shm_writer
            .expect_write()
            .withf(move |ceb: &ClockErrorBound| {
                ceb.void_after()
                    == ceb.as_of() + TimeSpec::from_duration(std::time::Duration::from_secs(1000))
                    && ceb.bound_nsec() == 0
                    && ceb.disruption_marker() == expected_disruption_marker
                    && ceb.max_drift_ppb() == expected_max_drift_ppb
                    && ceb.clock_status() == ClockStatus::Unknown
                    && ceb.clock_disruption_support_enabled() == clock_disruption_support_enabled
            })
            .times(1)
            .return_const(());
        let mut shm0 = Shm0::builder()
            .shm_writer(Box::new(shm_writer))
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .max_drift_ppb(expected_max_drift_ppb)
            .disruption_marker(expected_disruption_marker)
            .build();
        assert_eq!(shm0.disruption_marker, expected_disruption_marker);
        shm0.initialize();
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
                    && ceb.disruption_marker() == final_disruption_marker
                    && ceb.max_drift_ppb() == 10000000  // Clock params have 1% period error, in PPB
                    && ceb.clock_status() == ClockStatus::Disrupted
                    && ceb.clock_disruption_support_enabled() == clock_disruption_support_enabled
            })
            .times(1)
            .return_const(());

        let mut shm0 = Shm0::builder()
            .shm_writer(Box::new(shm_writer))
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(initial_disruption_marker)
            .build();
        assert_eq!(shm0.disruption_marker, initial_disruption_marker);
        shm0.handle_disruption(&clock_parameters, final_disruption_marker);
        assert_eq!(shm0.disruption_marker, final_disruption_marker);
    }
}
