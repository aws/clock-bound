//! Shared parent module for the two ClockBound SHM-segment writers.
//!
//! Each shared memory segment is written by its own concrete writer: [`Shm0`] writes the
//! `ClockErrorBoundV2` layout (used by ClockBound 2.0 clients, which lean on the kernel system
//! clock), and [`Shm1`] writes the `ClockErrorBoundV3` layout.
mod shm0;
mod shm1;
pub(crate) use shm0::Shm0;
pub(crate) use shm1::Shm1;

#[cfg(test)]
pub(crate) mod test_support {
    use crate::daemon::clock_parameters::ClockParameters;
    use crate::daemon::time::{Duration, Instant, TscCount, tsc::Period};
    use crate::shm::{ClockErrorBound, ShmWrite};

    mockall::mock! {
        pub ShmWriter {}
        impl ShmWrite for ShmWriter {
            fn write(&mut self, ceb: &ClockErrorBound);
        }
    }

    /// Helper function to create a test `ClockParameters`
    #[bon::builder]
    pub(crate) fn create_test_clock_parameters(
        clock_error_bound_nanos: i64,
        tsc_count: i64,
        time_nanos: i64,
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
}
