//! Write clockbound shared memory and manage clock state
use nix::sys::time::TimeSpec;
use tracing::info;

use crate::{
    daemon::{
        clock_parameters::ClockParameters,
        time::{
            Clock, Duration, Instant, clocks::MonotonicCoarse, inner::ClockOffsetAndRtt,
            instant::Utc,
        },
    },
    shm::{ClockErrorBound, ClockStatus, ShmWrite, ShmWriter},
};

/// The drift rate/maximal frequency error in parts-per-billion
/// for the `ClockBound` clock. The error bounds returned from the client
/// expand by this * the duration since `as_of`.
#[expect(unused)]
const FREQUENCY_ERROR_PPB: u32 = 15_000;

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
    clock_status: ClockStatus,
    clock_disruption_support_enabled: bool,
    shm_writer: T,
    max_drift_ppb: u32,
    disruption_marker: u64,
}

#[cfg_attr(test, mockall::automock)]
pub(crate) trait ClockStateWrite {
    fn handle_clock_parameters(
        &mut self,
        clock_parameters: &ClockParameters,
        clock_realtime_offset_and_rtt: ClockOffsetAndRtt<Utc>,
    );
    fn handle_disruption(&mut self, new_disruption_marker: u64);
}

impl<T: ShmWrite> ClockStateWrite for ClockStateWriter<T> {
    /// Handles `ClockParameters` passed out from the `ClockSyncAlgorithm` selector.
    ///
    /// # Panics
    /// Panics if error bound calculated exceeds `i64::MAX`
    fn handle_clock_parameters(
        &mut self,
        // This is needed to tell ClockAdjust what frequency to use.
        clock_parameters: &ClockParameters,
        // This is needed to tell ClockAdjust what phase offset to use.
        clock_realtime_offset_and_rtt: ClockOffsetAndRtt<Utc>,
    ) {
        // If we've received `ClockParameters`, it means we were able to construct a valid `ClockBound` clock.
        // Thus, mark ourselves as synchronized.
        self.clock_status = ClockStatus::Synchronized;
        let bound = get_bound(clock_parameters, clock_realtime_offset_and_rtt);
        // Unwrap safety: sane error bound should be less than `i64::MAX`
        let bound_nsec = i64::try_from(bound.as_nanos()).unwrap();
        // TODO: we should normally grab the `as_of` timestamp from the `ClockParameters` themselves,
        // e.g. use the `time` at which they were valid.
        // However, the client implementation today reads from `CLOCK_MONOTONIC_COARSE`. The `ClockParameters`
        // readings are not necessarily going to be monotonic nor aligned with `CLOCK_MONOTONIC_COARSE`.
        // If we write an `as_of` that is ahead of `CLOCK_MONOTONIC_COARSE`, we could see `ShmError::CausalityBreach` for client.
        // For the sake of backwards compatibility, we will have our initial/alpha release continue to work
        // using `CLOCK_MONOTONIC_COARSE`.
        let as_of = MonotonicCoarse.get_time();
        self.write_shm(as_of, bound_nsec);
    }

    /// Handle a clock disruption event
    ///
    /// Call this function after the system detects a VMClock disruption event.
    ///
    /// It will go through and clear the state (like startup).
    fn handle_disruption(&mut self, new_disruption_marker: u64) {
        // Use the destructure pattern to get a mutable reference to each item.
        //
        // This makes it a compilation error if we add a new field to Self without handling it here
        let Self {
            clock_status,
            clock_disruption_support_enabled: _,
            shm_writer: _,
            max_drift_ppb: _,
            disruption_marker,
        } = self;
        *clock_status = ClockStatus::Disrupted;
        *disruption_marker = new_disruption_marker;
        let as_of = MonotonicCoarse.get_time();
        info!("Writing `ClockStatus::Disrupted` to SHM with 0 `bound_nsec`");
        self.write_shm(as_of, 0); // We're writing that we're disrupted anyways, so the `bound_nsec` value should be useless here, 0 is ok
        tracing::info!("Handled clock disruption event");
    }
}

#[bon::bon]
impl<T: ShmWrite> ClockStateWriter<T> {
    #[builder]
    pub fn new(
        clock_disruption_support_enabled: bool,
        shm_writer: T,
        max_drift_ppb: u32,
        disruption_marker: u64,
    ) -> Self {
        Self {
            clock_status: ClockStatus::Unknown,
            clock_disruption_support_enabled,
            shm_writer,
            max_drift_ppb,
            disruption_marker,
        }
    }

    fn write_shm(&mut self, as_of: Instant, bound_nsec: i64) {
        let void_after = as_of + Duration::from_secs(1000);
        // TODO: It may be worthwhile to add to this max drift ppb base the following components:
        // - any slew rate for phase correction, since kernel clocks are used on client side
        // - error inherent to our frequency calculation e.g. `period_max_error`
        let max_drift_ppb = self.max_drift_ppb;
        let ceb: ClockErrorBound = ClockErrorBound::new(
            // Unwrap safety: unlikely to fail for any value for the distant future,
            // `i128` -> `i64` conversion would fail at 9_223_372_036_854_775_807 seconds
            TimeSpec::try_from(as_of).unwrap(),
            // Unwrap safety: unlikely to fail for any value for the distant future,
            // `i128` -> `i64` conversion would fail at 9_223_372_036_854_775_807 seconds
            TimeSpec::try_from(void_after).unwrap(),
            bound_nsec,
            self.disruption_marker,
            max_drift_ppb,
            self.clock_status,
            self.clock_disruption_support_enabled,
        );
        self.shm_writer.write(&ceb);
    }
}

/// Calculate the `ClockErrorBound` `bound_nsec` value. This is used to calculate
/// the `earliest` and `latest` readings from a `now` call.
///
/// # Arguments
/// * `clock_parameters` - Clock Parameters of best clock chosen by `ClockBound`
/// * `clock_realtime_offset_and_rtt` - offset and round trip time of a measurement of offset between `ClockBound` clock
///   and `CLOCK_REALTIME`
fn get_bound(
    clock_parameters: &ClockParameters,
    clock_realtime_offset_and_rtt: ClockOffsetAndRtt<Utc>,
) -> Duration {
    let realtime_to_clockbound_measured_offset = clock_realtime_offset_and_rtt.offset();
    let measurement_rtt = clock_realtime_offset_and_rtt.rtt();
    let bound_between_realtime_and_clockbound =
        realtime_to_clockbound_measured_offset + measurement_rtt / 2;
    clock_parameters.clock_error_bound + bound_between_realtime_and_clockbound
}

#[cfg(test)]
mod tests {
    use mockall::mock;
    use rstest::rstest;

    use crate::daemon::time::{Duration, TscCount, tsc::Period};

    use super::*;

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
        }
    }

    /// Helper function to create a test ClockOffsetAndRtt
    #[bon::builder]
    fn create_test_clock_offset_and_rtt(
        offset_nanos: i128,
        rtt_nanos: i128,
    ) -> ClockOffsetAndRtt<Utc> {
        ClockOffsetAndRtt::new(
            Duration::from_nanos(offset_nanos),
            Duration::from_nanos(rtt_nanos),
        )
    }

    #[rstest]
    #[case::synchronized(ClockStatus::Synchronized)]
    #[case::unknown(ClockStatus::Unknown)]
    #[case::free_running(ClockStatus::FreeRunning)]
    #[case::disrupted(ClockStatus::Disrupted)]
    fn test_write_shm(#[case] clock_status: ClockStatus) {
        let as_of = MonotonicCoarse.get_time();
        let bound_nsec = 1234;
        let max_drift_ppb = 234;
        let disruption_marker = 345;
        let clock_disruption_support_enabled = true;
        let expected_ceb = ClockErrorBound::new(
            TimeSpec::try_from(as_of).unwrap(),
            TimeSpec::try_from(as_of + Duration::from_secs(1000)).unwrap(),
            bound_nsec,
            disruption_marker,
            max_drift_ppb,
            clock_status,
            clock_disruption_support_enabled,
        );
        let mut shm_writer = MockShmWriter::new();
        shm_writer
            .expect_write()
            .withf(move |ceb: &ClockErrorBound| expected_ceb == *ceb)
            .times(1)
            .return_const(());
        let mut clock_state_writer = ClockStateWriter::builder()
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .shm_writer(shm_writer)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(disruption_marker)
            .build();
        clock_state_writer.clock_status = clock_status;
        clock_state_writer.write_shm(as_of, bound_nsec);
    }

    #[test]
    fn test_handle_clock_parameters() {
        let clock_parameters = create_test_clock_parameters()
            .clock_error_bound_nanos(1000)
            .tsc_count(1000)
            .time_nanos(1_000_000_000)
            .call();
        let clock_realtime_offset_and_rtt = create_test_clock_offset_and_rtt()
            .offset_nanos(1000)
            .rtt_nanos(500)
            .call();
        let clock_disruption_support_enabled = false;
        let max_drift_ppb = 0;
        let disruption_marker = 0;
        let mut shm_writer = MockShmWriter::new();
        shm_writer
            .expect_write()
            .withf(move |ceb: &ClockErrorBound| {
                ceb.void_after()
                    == ceb.as_of() + TimeSpec::from_duration(std::time::Duration::from_secs(1000))
                    && ceb.bound_nsec() == 2250
                    && ceb.disruption_marker() == disruption_marker
                    && ceb.max_drift_ppb() == max_drift_ppb
                    && ceb.clock_status() == ClockStatus::Synchronized // If we're getting clock parameters, we're "Synchronized"
                    && ceb.clock_disruption_support_enabled() == clock_disruption_support_enabled
            })
            .times(1)
            .return_const(());
        let mut clock_state_writer = ClockStateWriter::builder()
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .shm_writer(shm_writer)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(disruption_marker)
            .build();
        clock_state_writer
            .handle_clock_parameters(&clock_parameters, clock_realtime_offset_and_rtt);
    }

    #[test]
    #[should_panic]
    fn test_handle_clock_parameters_panic_on_overflow_error_bound() {
        let clock_parameters = create_test_clock_parameters()
            .clock_error_bound_nanos(i64::MAX as i128 + 1) // overflowing CEB
            .tsc_count(1000)
            .time_nanos(1_000_000_000)
            .call();
        let clock_realtime_offset_and_rtt = create_test_clock_offset_and_rtt()
            .offset_nanos(1000)
            .rtt_nanos(500)
            .call();
        let clock_disruption_support_enabled = false;
        let max_drift_ppb = 0;
        let disruption_marker = 0;
        let mut shm_writer = MockShmWriter::new();
        shm_writer.expect_write().never();
        let mut clock_state_writer = ClockStateWriter::builder()
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .shm_writer(shm_writer)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(disruption_marker)
            .build();
        clock_state_writer
            .handle_clock_parameters(&clock_parameters, clock_realtime_offset_and_rtt);
    }

    #[rstest]
    #[case(
        create_test_clock_parameters()
            .clock_error_bound_nanos(0)
            .tsc_count(1000)
            .time_nanos(1_000_000_000)
            .call(),
        create_test_clock_offset_and_rtt()
            .offset_nanos(0)
            .rtt_nanos(0)
            .call(),
        Duration::from_nanos(0),
    )]
    #[case(
        create_test_clock_parameters()
            .clock_error_bound_nanos(1000)
            .tsc_count(2000)
            .time_nanos(1_000_000_000)
            .call(),
        create_test_clock_offset_and_rtt()
            .offset_nanos(1000)
            .rtt_nanos(500)
            .call(),
        Duration::from_nanos(2250),
    )]
    fn test_get_bound(
        #[case] clock_parameters: ClockParameters,
        #[case] clock_realtime_offset_and_rtt: ClockOffsetAndRtt<Utc>,
        #[case] expected: Duration,
    ) {
        let bound_nsec = get_bound(&clock_parameters, clock_realtime_offset_and_rtt);
        assert_eq!(bound_nsec, expected);
    }

    #[test]
    fn handle_disruption() {
        let clock_disruption_support_enabled = false;
        let max_drift_ppb = 0;
        let initial_disruption_marker = 0;
        let final_disruption_marker = 1;
        let mut shm_writer = MockShmWriter::new();
        shm_writer
            .expect_write()
            .withf(move |ceb: &ClockErrorBound| {
                ceb.void_after()
                    == ceb.as_of() + TimeSpec::from_duration(std::time::Duration::from_secs(1000))
                    && ceb.bound_nsec() == 0
                    && ceb.disruption_marker() == final_disruption_marker
                    && ceb.max_drift_ppb() == max_drift_ppb
                    && ceb.clock_status() == ClockStatus::Disrupted
                    && ceb.clock_disruption_support_enabled() == clock_disruption_support_enabled
            })
            .times(1)
            .return_const(());
        let mut clock_state_writer = ClockStateWriter::builder()
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .shm_writer(shm_writer)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(initial_disruption_marker)
            .build();
        assert_eq!(clock_state_writer.clock_status, ClockStatus::Unknown);
        assert_eq!(
            clock_state_writer.disruption_marker,
            initial_disruption_marker
        );
        clock_state_writer.handle_disruption(final_disruption_marker);
        assert_eq!(
            clock_state_writer.disruption_marker,
            final_disruption_marker
        );
    }
}
