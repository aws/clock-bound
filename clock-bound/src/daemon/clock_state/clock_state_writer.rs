//! Write clockbound shared memory and manage clock state
use nix::sys::time::TimeSpec;
use tracing::info;

use crate::{
    daemon::{
        clock_parameters::ClockParameters,
        io::tsc::ReadTscImpl,
        time::{
            ClockExt, Duration, Instant, TscCount,
            clocks::{ClockBound, RealTime},
            tsc::{Period, Skew},
        },
    },
    shm::{
        ClockErrorBound, ClockErrorBoundGeneric, ClockErrorBoundLayoutVersion, ClockStatus,
        ShmWrite, ShmWriter,
    },
};

/// Newtype wrapper around `ShmWriter` so we can implement `Send` + `Sync`,
/// and thus construct this as part of an async task.
/// `ShmWriter` itself does not implement these because of its usage of a raw
/// pointer, but semantics in our daemon are that only one such `ShmWriter` should exist
/// at all, so `SafeShmWriter` does that for us.
pub struct SafeShmWriter(ShmWriter);
impl SafeShmWriter {
    pub fn new(shm_writer: ShmWriter) -> Self {
        Self(shm_writer)
    }
}
impl ShmWrite for SafeShmWriter {
    fn write(&mut self, ceb: &ClockErrorBound) {
        self.0.write(ceb);
    }
}
unsafe impl Send for SafeShmWriter {}
unsafe impl Sync for SafeShmWriter {}

pub struct ClockStateWriter<T> {
    clock_disruption_support_enabled: bool,
    // Writer to the */shm0 memory segment path
    shm_writer_0: T,
    // Writer to the */shm1 memory segment path
    shm_writer_1: T,
    max_drift_ppb: u32,
    disruption_marker: u64,
}

#[cfg_attr(test, mockall::automock)]
pub trait ClockStateWrite: Send + Sync {
    fn initialize_ceb_v2_shm(&mut self);
    fn handle_clock_parameters_shm1(
        &mut self,
        clock_parameters: &ClockParameters,
        clock_status: ClockStatus,
    );
    fn handle_clock_parameters_shm0(
        &mut self,
        clock_parameters: &ClockParameters,
        clock_status: ClockStatus,
    );
    fn handle_disruption(&mut self, clock_parameters: &ClockParameters, new_disruption_marker: u64);
}

impl<T: ShmWrite + Send + Sync> ClockStateWrite for ClockStateWriter<T> {
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
    /// This is a minor change in behavior compared to how ClockBound 2.0 manages the SHM segment
    /// upon restart.
    fn initialize_ceb_v2_shm(&mut self) {
        info!("Initializing SHM segment to status `ClockStatus::Unknown` and zeroing other fields");
        let clock_parameters = ClockParameters {
            tsc_count: TscCount::new(0),
            time: Instant::new(0),
            clock_error_bound: Duration::new(0),
            period: Period::from_seconds(0.0),
            period_max_error: Period::from_seconds(0.0),
            as_of_monotonic: Instant::new(0),
        };
        self.write_shm0(&clock_parameters, ClockStatus::Unknown);
    }

    /// Handles `ClockParameters` passed out from the `ClockSyncAlgorithm` selector for SHM1 (`ClockErrorBoundV3`)
    fn handle_clock_parameters_shm1(
        &mut self,
        clock_parameters: &ClockParameters,
        clock_status: ClockStatus,
    ) {
        self.write_shm1(clock_parameters, clock_status);
    }

    /// Handles `ClockParameters` passed out from the `ClockSyncAlgorithm` selector for SHM0 (`ClockErrorBoundV2`)
    ///
    /// # Panics
    /// Panics if error bound calculated exceeds `i64::MAX`
    fn handle_clock_parameters_shm0(
        &mut self,
        clock_parameters: &ClockParameters,
        clock_status: ClockStatus,
    ) {
        // The kernel system clock is used on `ClockErrorBoundV2` client reads, so the error between that and
        // the `ClockBound` clock calculated from `ClockParameters` needs to be added in, thus we measure it here
        let clockbound_clock = ClockBound::new(clock_parameters.clone(), ReadTscImpl);
        let offset_and_rtt = clockbound_clock.get_offset_and_rtt(&RealTime);

        let clock_error_bound = clock_parameters.clock_error_bound
            + offset_and_rtt.offset().abs()
            + (offset_and_rtt.rtt() / 2);

        let shm0_clock_parameters = ClockParameters {
            clock_error_bound,
            ..clock_parameters.clone()
        };
        self.write_shm0(&shm0_clock_parameters, clock_status);
    }

    /// Handle a clock disruption event
    ///
    /// Call this function after the system detects a VMClock disruption event.
    ///
    /// It will go through and clear the state (like startup).
    fn handle_disruption(
        &mut self,
        clock_parameters: &ClockParameters,
        new_disruption_marker: u64,
    ) {
        // Use the destructure pattern to get a mutable reference to each item.
        //
        // This makes it a compilation error if we add a new field to Self without handling it here
        let Self {
            clock_disruption_support_enabled: _,
            shm_writer_0: _,
            shm_writer_1: _,
            max_drift_ppb: _,
            disruption_marker,
        } = self;
        *disruption_marker = new_disruption_marker;

        // Write to the latest and greatest SHM segment first, older one(s) afterwards.
        self.write_shm1(clock_parameters, ClockStatus::Disrupted);
        self.write_shm0(clock_parameters, ClockStatus::Disrupted);

        tracing::info!("Handled clock disruption event");
    }
}

#[bon::bon]
impl<T: ShmWrite> ClockStateWriter<T> {
    #[builder]
    pub fn new(
        clock_disruption_support_enabled: bool,
        shm_writer_0: T,
        shm_writer_1: T,
        max_drift_ppb: u32,
        disruption_marker: u64,
    ) -> Self {
        Self {
            clock_disruption_support_enabled,
            shm_writer_0,
            shm_writer_1,
            max_drift_ppb,
            disruption_marker,
        }
    }

    /// Write out to the ClockBound daemon shared memory segment shm0.
    ///
    /// Writes the latest supported layout version (V2) of the shared memory segment.
    fn write_shm0(&mut self, clock_parameters: &ClockParameters, clock_status: ClockStatus) {
        // XXX: here as_of is the CLOCK_MONOTONIC_COARSE timestamp taken *before* the clock
        // parameters were computed.
        //
        // Unwrap safety: unlikely to fail for any value for the distant future,
        // `i128` -> `i64` conversion would fail at 9_223_372_036_854_775_807 seconds
        let as_of = TimeSpec::try_from(clock_parameters.as_of_monotonic).unwrap();
        let void_after = as_of + TimeSpec::new(1000, 0);

        let bound_nsec = i64::try_from(clock_parameters.clock_error_bound.as_nanos()).unwrap();

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
                "Software skew is too large to be expressed as a ppb, skipping writing to SHM"
            );
            return;
        };
        let max_drift_ppb = self.max_drift_ppb + software_skew_ppb;

        // Build the ClockErrorBound::V2 layout and write it out.
        let ceb = ClockErrorBoundGeneric::builder()
            .as_of(as_of)
            .void_after(void_after)
            .bound_nsec(bound_nsec)
            .disruption_marker(self.disruption_marker)
            .max_drift_ppb(max_drift_ppb)
            .clock_status(clock_status)
            .clock_disruption_support_enabled(self.clock_disruption_support_enabled)
            .build(ClockErrorBoundLayoutVersion::V2);

        self.shm_writer_0.write(&ceb);
    }

    /// Write out to the ClockBound daemon shared memory segment shm1.
    ///
    /// Writes the latest supported layout version (V3) of the shared memory segment.
    fn write_shm1(&mut self, clock_parameters: &ClockParameters, clock_status: ClockStatus) {
        let bound_nsec = i64::try_from(clock_parameters.clock_error_bound.as_nanos()).unwrap();

        // Unwrap safety: unlikely to fail for any value for the distant future,
        // `i128` -> `i64` conversion would fail at 9_223_372_036_854_775_807 seconds
        let as_of = TimeSpec::try_from(clock_parameters.time).unwrap();
        let void_after = as_of + TimeSpec::new(1000, 0);

        #[allow(clippy::cast_possible_truncation)]
        #[allow(clippy::cast_sign_loss)]
        let ceb = ClockErrorBoundGeneric::builder()
            .as_of_tsc(clock_parameters.tsc_count.get() as u64)
            .as_of(as_of)
            .void_after(void_after)
            .bound_nsec(bound_nsec)
            .period(clock_parameters.period.get())
            .period_err(clock_parameters.period_max_error.get())
            .disruption_marker(self.disruption_marker)
            .max_drift_ppb(self.max_drift_ppb)
            .clock_status(clock_status)
            .clock_disruption_support_enabled(self.clock_disruption_support_enabled)
            .build(ClockErrorBoundLayoutVersion::V3);
        self.shm_writer_1.write(&ceb);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::time::{Duration, Instant, TscCount, tsc::Period};
    use mockall::mock;
    use rstest::rstest;

    mock! {
        ShmWriter {}
        impl ShmWrite for ShmWriter {
            fn write(&mut self, ceb: &ClockErrorBound);
        }
    }

    /// Helper function to create a test ClockParameters
    #[bon::builder]
    fn create_test_clock_parameters(
        clock_error_bound_nanos: i128,
        tsc_count: i128,
        time_nanos: i128,
    ) -> ClockParameters {
        ClockParameters {
            tsc_count: TscCount::new(tsc_count),
            time: Instant::from_nanos(time_nanos),
            clock_error_bound: Duration::from_nanos(clock_error_bound_nanos),
            period: Period::from_seconds(1e-9),
            period_max_error: Period::from_seconds(1e-11),
            as_of_monotonic: Instant::from_nanos(time_nanos),
        }
    }

    #[rstest]
    #[case::synchronized(ClockStatus::Synchronized)]
    #[case::unknown(ClockStatus::Unknown)]
    #[case::free_running(ClockStatus::FreeRunning)]
    #[case::disrupted(ClockStatus::Disrupted)]
    fn test_write_shm(#[case] clock_status: ClockStatus) {
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
        let mut shm_writer_0 = MockShmWriter::new();
        shm_writer_0
            .expect_write()
            .withf(move |ceb: &ClockErrorBound| expected_ceb_v2 == *ceb)
            .times(1)
            .return_const(());

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
            .period_err(1e-11)
            .bound_nsec(500)
            .disruption_marker(disruption_marker)
            .max_drift_ppb(max_drift_ppb)
            .clock_status(clock_status)
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .build(ClockErrorBoundLayoutVersion::V3);
        let mut shm_writer_1 = MockShmWriter::new();
        shm_writer_1
            .expect_write()
            .withf(move |ceb: &ClockErrorBound| expected_ceb_v3 == *ceb)
            .times(1)
            .return_const(());

        let mut clock_state_writer = ClockStateWriter::builder()
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .shm_writer_0(shm_writer_0)
            .shm_writer_1(shm_writer_1)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(disruption_marker)
            .build();
        clock_state_writer.write_shm0(&clock_parameters, clock_status);
        clock_state_writer.write_shm1(&clock_parameters, clock_status);
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

        let mut shm_writer_0 = MockShmWriter::new();
        shm_writer_0
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

        let mut shm_writer_1 = MockShmWriter::new();
        shm_writer_1.expect_write().never();

        let mut clock_state_writer = ClockStateWriter::builder()
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .shm_writer_0(shm_writer_0)
            .shm_writer_1(shm_writer_1)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(disruption_marker)
            .build();
        clock_state_writer.handle_clock_parameters_shm0(&clock_parameters, clock_status);
    }

    #[test]
    #[should_panic]
    fn handle_clock_parameters_shm0_panic_on_overflow_error_bound() {
        let clock_parameters = create_test_clock_parameters()
            .clock_error_bound_nanos(i64::MAX as i128 + 1) // overflowing CEB
            .tsc_count(1000)
            .time_nanos(1_000_000_000)
            .call();
        let clock_disruption_support_enabled = false;
        let max_drift_ppb = 0;
        let disruption_marker = 0;
        let mut shm_writer_0 = MockShmWriter::new();
        shm_writer_0.expect_write().never();
        let mut shm_writer_1 = MockShmWriter::new();
        shm_writer_1.expect_write().never();
        let mut clock_state_writer = ClockStateWriter::builder()
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .shm_writer_0(shm_writer_0)
            .shm_writer_1(shm_writer_1)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(disruption_marker)
            .build();
        clock_state_writer
            .handle_clock_parameters_shm0(&clock_parameters, ClockStatus::Synchronized);
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

        let mut shm_writer_0 = MockShmWriter::new();
        shm_writer_0.expect_write().never();

        let mut shm_writer_1 = MockShmWriter::new();
        shm_writer_1
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

        let mut clock_state_writer = ClockStateWriter::builder()
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .shm_writer_0(shm_writer_0)
            .shm_writer_1(shm_writer_1)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(disruption_marker)
            .build();
        clock_state_writer.handle_clock_parameters_shm1(&clock_parameters, clock_status);
    }

    #[test]
    fn test_initialize_ceb_v2_shm() {
        let clock_disruption_support_enabled = false;
        let expected_max_drift_ppb = 15_000;
        let expected_disruption_marker = 0;
        let mut shm_writer_0 = MockShmWriter::new();
        shm_writer_0
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
        let mut shm_writer_1 = MockShmWriter::new();
        shm_writer_1.expect_write().never();
        let mut clock_state_writer = ClockStateWriter::builder()
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .shm_writer_0(shm_writer_0)
            .shm_writer_1(shm_writer_1)
            .max_drift_ppb(expected_max_drift_ppb)
            .disruption_marker(expected_disruption_marker)
            .build();
        assert_eq!(
            clock_state_writer.disruption_marker,
            expected_disruption_marker
        );
        clock_state_writer.initialize_ceb_v2_shm();
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

        let mut shm_writer_0 = MockShmWriter::new();
        shm_writer_0
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

        let mut shm_writer_1 = MockShmWriter::new();
        shm_writer_1
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

        let mut clock_state_writer = ClockStateWriter::builder()
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .shm_writer_0(shm_writer_0)
            .shm_writer_1(shm_writer_1)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(initial_disruption_marker)
            .build();
        assert_eq!(
            clock_state_writer.disruption_marker,
            initial_disruption_marker
        );
        clock_state_writer.handle_disruption(&clock_parameters, final_disruption_marker);
        assert_eq!(
            clock_state_writer.disruption_marker,
            final_disruption_marker
        );
    }
}
