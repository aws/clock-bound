//! Clock Adjustment state machine.
//!
//! On initialization (when we receive valid `ClockParameters`), the adjuster measures the
//! phase offset between `CLOCK_REALTIME` and the `ClockBound` internal clock.
//!
//! - If `|offset| > Timex::MAX_PHASE_OFFSET` (500 ms), the clock is stepped
//!   (`apply_phase_correction_step`). This handles stale RTCs and booting to the Unix epoch.
//! - Otherwise the offset is fed into the kernel PLL (`apply_phase_correction_smooth`)
//!   for a slew. This covers the possible case of a daemon restart where the
//!   clock is already close.
//!
//! After either action the state moves to `Running`. In `Running` every tick measures
//! the offset the same way and submits it to the kernel PLL via `apply_phase_correction_smooth`.
//! The kernel slews at a constant rate over the following second, re-calculated each
//! second, decaying logarithmically.
//!
//! ```text
//!   ┌─────────────┐
//!   │ Initializing│  No adjustments made yet.
//!   └──────┬──────┘
//!          │ first clock_params: measure offset
//!          │   |offset| > MAX_PHASE_OFFSET → apply_phase_correction_step
//!          │   |offset| ≤ MAX_PHASE_OFFSET → apply_phase_correction_smooth
//!          v
//!   ┌─────────┐
//!   │ Running │  Every tick: measure offset, apply_phase_correction_smooth → kernel PLL.
//!   └─────────┘
//! ```
use tracing::{debug, info};

use crate::daemon::{
    clock_parameters::ClockParameters,
    io::tsc::ReadTscImpl,
    time::{
        ClockExt,
        clocks::{ClockBound, RealTime},
        timex::Timex,
    },
};

use super::ntp_adjtime::NtpAdjTimeExt;

// ─── State types ─────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub(super) struct Initializing;

#[derive(Debug, PartialEq)]
pub(super) struct Running;

#[derive(Debug, PartialEq)]
pub(super) enum State {
    Initializing(Initializing),
    Running(Running),
}

// ─── Transitions ─────────────────────────────────────────────────────────────

impl Initializing {
    /// On the first tick, measure the phase offset and either step or smooth-slew
    /// `CLOCK_REALTIME` into alignment, then move to `Running`.
    #[must_use]
    #[allow(clippy::unused_self)]
    pub(super) fn to_running(
        &self,
        ntp_adjtime: &impl NtpAdjTimeExt,
        clock_params: &ClockParameters,
    ) -> Running {
        let clockbound = ClockBound::new(clock_params.clone(), ReadTscImpl);
        let offset = clockbound.get_offset_and_rtt(&RealTime).offset();
        let threshold = Timex::MAX_PHASE_OFFSET;
        if offset.abs() > threshold {
            ntp_adjtime
                .apply_phase_correction_step(offset)
                .expect("failed to step clock on startup");
            info!("Clock stepped by {offset:?} (|offset| > {threshold:?}); entering Running");
        } else {
            ntp_adjtime
                .apply_phase_correction_smooth(offset)
                .expect("failed to smooth-apply phase correction on startup");
            info!(
                "Clock smooth-slewed by {offset:?} (|offset| <= {threshold:?}); entering Running"
            );
        }
        info!("Initializing now transitioning to Running");
        Running
    }
}

impl Running {
    /// On every subsequent tick, measure the offset and feed it into the kernel PLL.
    #[must_use]
    #[allow(clippy::unused_self)]
    pub(super) fn apply_offset(
        &self,
        ntp_adjtime: &impl NtpAdjTimeExt,
        clock_params: &ClockParameters,
    ) -> Running {
        let clockbound = ClockBound::new(clock_params.clone(), ReadTscImpl);
        let offset = clockbound.get_offset_and_rtt(&RealTime).offset();
        let result = ntp_adjtime
            .apply_phase_correction_smooth(offset)
            .expect("failed to smooth-apply phase correction");
        debug!(
            "ntp_adjtime apply_phase_correction_smooth returned kernel state: {:?}",
            result
        );
        Running
    }
}

#[cfg(test)]
mod test {
    use crate::daemon::{
        clock_state::clock_adjust::ntp_adjtime::MockNtpAdjTimeExt,
        time::{
            Clock, Duration, TscCount,
            tsc::{Frequency, Period},
        },
    };

    use super::*;

    /// Build clock parameters anchored to the current TSC and wall clock, then
    /// shift `time` by `target_offset` so `ClockBound` reads approximately
    /// `RealTime + target_offset`.
    fn clock_params_with_offset(target_offset: Duration) -> ClockParameters {
        let tsc_now = TscCount::new(crate::daemon::io::tsc::read_timestamp_counter_begin() as i64);
        let realtime_now = crate::daemon::time::clocks::RealTime.get_time();
        ClockParameters {
            tsc_count: tsc_now,
            period: Period::from_frequency(Frequency::from_hz(1_000_000_000.0)),
            time: realtime_now + target_offset,
            clock_error_bound: Duration::new(0),
            period_max_error: Period::from_seconds(0.0),
            as_of_monotonic: realtime_now,
        }
    }

    #[test]
    fn initializing_small_offset_smooth_slews_to_running() {
        let mut mock = MockNtpAdjTimeExt::new();
        mock.expect_apply_phase_correction_smooth()
            .once()
            .returning(|_| Ok(crate::daemon::time::timex::Timex::retrieve()));

        let state =
            Initializing.to_running(&mock, &clock_params_with_offset(Duration::from_millis(100)));
        assert!(matches!(state, Running));
    }

    #[test]
    fn initializing_large_offset_steps_to_running() {
        let mut mock = MockNtpAdjTimeExt::new();
        mock.expect_apply_phase_correction_step()
            .once()
            .returning(|_| Ok(crate::daemon::time::timex::Timex::retrieve()));

        let state =
            Initializing.to_running(&mock, &clock_params_with_offset(Duration::from_secs(1)));
        assert!(matches!(state, Running));
    }

    #[test]
    fn running_calls_apply_phase_correction_smooth() {
        let mut mock = MockNtpAdjTimeExt::new();
        mock.expect_apply_phase_correction_smooth()
            .once()
            .returning(|_| Ok(crate::daemon::time::timex::Timex::retrieve()));

        let running = Running;
        let next =
            running.apply_offset(&mock, &clock_params_with_offset(Duration::from_millis(50)));
        assert!(matches!(next, Running));
    }
}
