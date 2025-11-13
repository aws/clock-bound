//! `ClockBound` tries to make `CLOCK_REALTIME` follow UTC as closely as possible.
//! It has its own notion of a "best internal clock" output by the `ClockSyncAlgorithm` and selector,
//! which is expressed as a set of `ClockParameters`, and reading that clock is done via
//! using those `ClockParameters` + corresponding TSC reads, similar to `clock_gettime` VDSO implementation.
//! To make `CLOCK_REALTIME` follow UTC, we make it follow `ClockBound` internal clock by steering it via
//! frequency and phase corrections.
//!
//! Frequency corrections are applied via `ntp_adjtime` - the frequency to apply is calculated by comparing
//! two `ClockSnapshot`s with timestamps of `CLOCK_REALTIME` along with `ClockBound` internal clock, calculating the relative
//! frequency, and applying the relative frequency change to the currently used `CLOCK_REALTIME` frequency (gathered
//! via `ntp_adjtime` as part of the `ClockSnapshot`.
//!
//! Phase corrections are applied via `ntp_adjtime` as well, and are a simpler task - we simply use the old trick
//! of interleaved reads to compare the offset of two clocks (offset of `ClockBound` w.r.t `CLOCK_REALTIME`) and apply
//! that as the phase correction to `CLOCK_REALTIME`, using the PLL slewing method in kernel. An alternative approach
//! to fixing phase corrections could involve temporary slewing of the clock frequency, but can risk overshooting, or
//! leaving the system in a bad state if in the middle of a slew and the daemon is terminated.
//!
//! We have to use a decently complex state machine to manage our clock adjustments - our method for frequency estimate
//! can be impacted by the PLL effect itself, which we unfortunately cannot perfectly avoid or compensate for (since the amount
//! the PLL has slewed the clock cannot be grabbed directly beyond a second's granularity, and estimation is unreliable). Thus,
//! we make it explicit that the snapshots of the clock we make are NOT overlapping with any PLL slew, by halting the slew.
//!
//! What we end up getting is a stable frequency calculation (which should not change often unless conditions change quickly) with
//! its error being solely due to `ClockParameters` period error itself as well as measurement error from `ClockSnapshot`s RTT of TSC and `ClockRealTime`
//! reads, and phase correction that converges and does not overshoot.
//!
//!```text
//!               Clock Adjustment State Machine
//!                    ┌─────────────────┐     ┌─────────────────┐
//!                    │   Initialized   │     |    Disrupted    | (any other state can immediately transition to Disrupted
//!                    │ (unreliable)    │     |  (unreliable)   | and clock adjustments are reset in its handler)
//!                    └─────────────────┘     └─────────────────┘
//!                             |                        |
//!                             |  halt PLL              | halt PLL
//!                             |  step clock            |
//!                             v                        |
//!                ┌──────────────────────────┐          |
//!                │ InitialPhaseCorrectHalted│ <────────┘
//!                │       (unreliable)       │
//!                └──────────────────────────┘
//!                             | wait PHASE_CORRECTION_HALT_DURATION, then take snapshot A
//!                             |
//!                             v
//!                ┌──────────────────────────┐
//!                │ InitialSnapshotRetrieved │
//!                │ (unreliable)             │
//!                └──────────────────────────┘
//!                             | wait INITIAL_SNAPSHOT_RETRIEVED_DURATION then
//!                             | take snapshot B +
//!                             | calc frequency +
//!                             v apply corrections
//!                    ┌─────────────────┐
//!                    │  ClockAdjusted  │
//!                    │ halt the PLL    │
//!        ┌───────────│                 │<────────┐
//!        │           │ (reliable)      │         |
//!        │           │                 │         | wait SNAPSHOT_RETRIEVED_DURATION, then
//!        │           └─────────────────┘         │ take snapshot B +
//!        │ wait PHASE_CORRECTING_DURATION        | calc frequency +
//!        │ then halt PLL                         | apply corrections
//!        │                                       |
//!        v                                       |
//! ┌──────────────────┐                  ┌──────────────────┐
//! │PhaseCorrectHalted│                  │ SnapshotRetrieved│
//! |   (reliable).    |                  │ (reliable)       │
//! └──────────────────┘                  └──────────────────┘
//!        │                                        ^
//!        │                                        │
//!        └────────────────────────────────────────┘
//!    wait PHASE_CORRECTION_HALT_DURATION, then take snapshot A
//! ```
use tracing::debug;

use crate::daemon::{
    clock_parameters::ClockParameters,
    event::SystemClockMeasurement,
    io::tsc::ReadTscImpl,
    time::{
        ClockExt, Duration,
        clocks::{ClockBound, RealTime},
        timex::Timex,
        tsc::Skew,
    },
};

use super::ntp_adjtime::NtpAdjTimeExt;

#[derive(Debug, PartialEq)]
pub(super) enum State {
    /// State indicating that the clock has been disrupted.
    Disrupted(Disrupted),
    /// Our initial state.
    /// When we get a new event (which require some `ClockParameters`),
    /// we step the clock, and transition out of this state, to `InitialPhaseCorrectHalted`.
    ///
    /// The clock is not yet reliable.
    Initialized(Initialized),
    /// State after we transition from `Initializing`.
    /// Any Phase Correction supplied to the kernel has been halted, so that we can
    /// take a snapshot of `CLOCK_REALTIME` outside of any slew, to use for our frequency calculation.
    /// After `PHASE_CORRECTION_HALT_DURATION`, we take a `ClockSnapshot` (Snapshot A) and transition to `InitialSnapshotRetrieved`.
    ///
    /// The clock is not yet reliable.
    InitialPhaseCorrectHalted(InitialPhaseCorrectHalted),
    /// State after we have taken an initial `ClockSnapshot` to be used for a relative frequency estimation
    /// (getting the frequency of `CLOCK_REALTIME` w.r.t. `ClockBound` internal clock,
    /// so that we may adjust `CLOCK_REALTIME` to follow it)
    ///
    /// After `INITIAL_SNAPSHOT_A_DURATION`, we take another `ClockSnapshot` (Snapshot B), which we can then use for
    /// our relative frequency calculation. We additionally calculate the offset of `CLOCK_REALTIME` w.r.t `ClockBound`
    /// internal clock. With these two, we adjust the clock, and transition to `ClockAdjusted` state.
    ///
    /// The clock is not yet reliable.
    InitialSnapshotRetrieved(InitialSnapshotRetrieved),
    /// State after we have applied a frequency and phase correction to the clock.
    ///
    /// After `PHASE_CORRECTING_DURATION`, we have corrected a good portion of the phase offset using PLL correction.
    /// We halt the PLL correction and then transition to `PhaseCorrectHalted`, so that we can take another measurement
    /// of the frequency. From here, we are in steady state, and cycle through the states
    /// `ClockAdjusted` -> `PhaseCorrectHalted` -> `SnapshotRetrieved` -> `ClockAdjusted` -> [...].
    ///
    /// The clock is now reliable.
    ClockAdjusted(ClockAdjusted),
    /// State after we transition from `ClockAdjusted`.
    /// Any Phase Correction supplied to the kernel has been halted, so that we can
    /// take a snapshot of `CLOCK_REALTIME` outside of any slew, to use for our frequency calculation.
    /// After `PHASE_CORRECTION_HALT_DURATION`, we take a `ClockSnapshot` (Snapshot A) and transition to `SnapshotRetrieved`.
    ///
    /// The clock is now reliable.
    PhaseCorrectHalted(PhaseCorrectHalted),
    /// State after we have taken an initial `ClockSnapshot` to be used for a relative frequency estimation
    /// (getting the frequency of `CLOCK_REALTIME` w.r.t. `ClockBound` internal clock,
    /// so that we may adjust `CLOCK_REALTIME` to follow it)
    ///
    /// After `SNAPSHOT_A_DURATION` (longer than `INITIAL_SNAPSHOT_A_DURATION`, to allow for a longer term frequency measurement),
    /// we take another `ClockSnapshot` (Snapshot B), which we can then use for
    /// our relative frequency calculation. We additionally calculate the offset of `CLOCK_REALTIME` w.r.t `ClockBound`
    /// internal clock. With these two, we adjust the clock, and transition to `ClockAdjusted` state.
    ///
    /// The clock is now reliable.
    SnapshotRetrieved(SnapshotRetrieved),
}
#[derive(Debug, PartialEq)]
pub(super) struct Disrupted;
impl Disrupted {
    #[must_use]
    #[allow(clippy::unused_self)]
    pub(super) fn to_initial_phase_correct_halted(
        &self,
        ntp_adjtime: &impl NtpAdjTimeExt,
    ) -> InitialPhaseCorrectHalted {
        // Unwrap safety: If we can't adjust the clock, better to panic,
        // else we are outside the expectations of our state machine
        ntp_adjtime
            .apply_phase_correction(Duration::from_secs(0))
            .expect("failed to halt phase correction");
        debug!("Disrupted now transitioning to InitialPhaseCorrectHalted");
        InitialPhaseCorrectHalted {
            instant: tokio::time::Instant::now(),
        }
    }
}
#[derive(Debug, PartialEq)]
pub(super) struct Initialized;
impl Initialized {
    #[must_use]
    #[allow(clippy::unused_self)]
    pub(super) fn to_initial_phase_correct_halted(
        &self,
        ntp_adjtime: &impl NtpAdjTimeExt,
        clock_params: &ClockParameters,
    ) -> InitialPhaseCorrectHalted {
        // Unwrap safety: If we can't adjust the clock, better to panic,
        // else we are outside the expectations of our state machine
        ntp_adjtime
            .apply_phase_correction(Duration::from_secs(0))
            .expect("failed to halt phase correction");
        let clockbound_clock = ClockBound::new(clock_params.clone(), ReadTscImpl);
        // TODO: implement multiple attempts in case of latency increase
        let offset_and_rtt = clockbound_clock.get_offset_and_rtt(&RealTime);
        // Unwrap safety: If we can't adjust the clock, better to panic,
        // else we are outside the expectations of our state machine
        ntp_adjtime
            .step_clock(offset_and_rtt.offset())
            .expect("failed to step clock");
        debug!("Initialized now transitioning to InitialPhaseCorrectHalted");
        InitialPhaseCorrectHalted {
            instant: tokio::time::Instant::now(),
        }
    }
}
#[derive(Debug, PartialEq)]
pub(super) struct InitialPhaseCorrectHalted {
    pub(super) instant: tokio::time::Instant,
}
impl InitialPhaseCorrectHalted {
    /// Duration which we should stay in `InitialPhaseCorrectHalted`.
    /// Since PLL adjustment starts at the top of a second, this should take at least one second.
    /// It's possible the clock used for calculation of `Duration` in our async runtime
    /// runs a bit slower than this though, so we use 2 seconds to be safe.
    pub(super) const DURATION: tokio::time::Duration = tokio::time::Duration::from_secs(2);

    #[must_use]
    #[allow(clippy::unused_self)]
    pub(super) fn to_initial_snapshot_retrieved(
        &self,
        ntp_adjtime: &impl NtpAdjTimeExt,
    ) -> InitialSnapshotRetrieved {
        debug!("InitialPhaseCorrectHalted now transitioning to InitialSnapshotRetrieved");
        InitialSnapshotRetrieved {
            instant: tokio::time::Instant::now(),
            snapshot: ClockSnapshot::retrieve(ntp_adjtime),
        }
    }
}
#[derive(Debug, PartialEq)]
pub(super) struct InitialSnapshotRetrieved {
    pub(super) instant: tokio::time::Instant,
    pub(super) snapshot: ClockSnapshot,
}
impl InitialSnapshotRetrieved {
    /// Duration which we should stay in `InitialSnapshotRetrieved`. We calculate relative frequency adjustment to apply
    /// to have `CLOCK_REALTIME` match `ClockBound` clock rate over this duration, by grabbing two snapshots A and B
    /// and calculating the relative difference between the two.
    pub(super) const DURATION: tokio::time::Duration = tokio::time::Duration::from_secs(1);

    #[must_use]
    #[allow(clippy::unused_self)]
    pub(super) fn to_clock_adjusted(
        &self,
        ntp_adjtime: &impl NtpAdjTimeExt,
        clock_params: &ClockParameters,
        snapshot: &ClockSnapshot,
    ) -> ClockAdjusted {
        let new_snapshot = ClockSnapshot::retrieve(ntp_adjtime);
        let freq = calculate_frequency_correction(clock_params, snapshot, &new_snapshot);
        let clockbound_clock = ClockBound::new(clock_params.clone(), ReadTscImpl);
        // TODO: implement multiple attempts in case of latency increase
        let offset_and_rtt = clockbound_clock.get_offset_and_rtt(&RealTime);
        // Unwrap safety: If we can't adjust the clock, better to panic,
        // else we are outside the expectations of our state machine
        ntp_adjtime
            .adjust_clock(offset_and_rtt.offset(), freq)
            .expect("failed to adjust clock frequency and phase correction");
        debug!("InitialSnapshotRetrieved now transitioning to ClockAdjusted");
        ClockAdjusted {
            instant: tokio::time::Instant::now(),
        }
    }
}
#[derive(Debug, PartialEq)]
pub(super) struct ClockAdjusted {
    pub(super) instant: tokio::time::Instant,
}
impl ClockAdjusted {
    /// Duration which we should stay in `ClockAdjusted` (letting the PLL phase correction run).
    /// The amount of phase offset corrected via PLL slewing can be approximated based on PLL,
    /// to be `(1 - 0.75^n)` where `n` = seconds since PLL start.
    /// With 8 seconds, we slew approximately 90% of the offset we intended to.
    pub(super) const DURATION: tokio::time::Duration = tokio::time::Duration::from_secs(8);

    #[must_use]
    #[allow(clippy::unused_self)]
    pub(super) fn to_phase_correct_halted(
        &self,
        ntp_adjtime: &impl NtpAdjTimeExt,
    ) -> PhaseCorrectHalted {
        // Unwrap safety: If we can't adjust the clock, better to panic,
        // else we are outside the expectations of our state machine
        ntp_adjtime
            .apply_phase_correction(Duration::from_secs(0))
            .expect("failed to halt phase correction");
        debug!("ClockAdjusted now transitioning to PhaseCorrectHalted");
        PhaseCorrectHalted {
            instant: tokio::time::Instant::now(),
        }
    }
}
#[derive(Debug, PartialEq)]
pub(super) struct PhaseCorrectHalted {
    pub(super) instant: tokio::time::Instant,
}
impl PhaseCorrectHalted {
    /// Duration which we should stay in `PhaseCorrectHalted`.
    /// Since PLL adjustment starts at the top of a second, this should take at least one second.
    /// It's possible the clock used for calculation of `Duration` in our async runtime
    /// runs a bit slower than this though, so we use 2 seconds to be safe.
    pub(super) const DURATION: tokio::time::Duration = tokio::time::Duration::from_secs(2);

    #[must_use]
    #[allow(clippy::unused_self)]
    pub(super) fn to_snapshot_retrieved(
        &self,
        ntp_adjtime: &impl NtpAdjTimeExt,
    ) -> SnapshotRetrieved {
        debug!("PhaseCorrectHalted now transitioning to SnapshotRetrieved");
        SnapshotRetrieved {
            instant: tokio::time::Instant::now(),
            snapshot: ClockSnapshot::retrieve(ntp_adjtime),
        }
    }
}
#[derive(Debug, PartialEq)]
pub(super) struct SnapshotRetrieved {
    pub(super) instant: tokio::time::Instant,
    pub(super) snapshot: ClockSnapshot,
}
impl SnapshotRetrieved {
    /// Duration which we should stay in `SnapshotRetrieved`. We calculate relative frequency adjustment to apply
    /// to have `CLOCK_REALTIME` match `ClockBound` clock rate over this duration, by grabbing two snapshots A and B.
    pub(super) const DURATION: tokio::time::Duration = tokio::time::Duration::from_secs(10);

    #[must_use]
    #[allow(clippy::unused_self)]
    pub(super) fn to_clock_adjusted(
        &self,
        ntp_adjtime: &impl NtpAdjTimeExt,
        clock_params: &ClockParameters,
        snapshot: &ClockSnapshot,
    ) -> ClockAdjusted {
        let new_snapshot = ClockSnapshot::retrieve(ntp_adjtime);
        let freq = calculate_frequency_correction(clock_params, snapshot, &new_snapshot);
        let clockbound_clock = ClockBound::new(clock_params.clone(), ReadTscImpl);
        // TODO: implement multiple attempts in case of latency increase
        let offset_and_rtt = clockbound_clock.get_offset_and_rtt(&RealTime);
        // Unwrap safety: If we can't adjust the clock, better to panic,
        // else we are outside the expectations of our state machine
        ntp_adjtime
            .adjust_clock(offset_and_rtt.offset(), freq)
            .expect("failed to adjust clock frequency and phase correction");
        debug!("SnapshotRetrieved now transitioning to ClockAdjusted");
        ClockAdjusted {
            instant: tokio::time::Instant::now(),
        }
    }
}

/// Based on old and new snapshots of `CLOCK_REALTIME` timestamp and frequency and corresponding TSC reads,
/// alongside `ClockParameters`, we calculate a frequency correction (`Skew`) to supply to our `adjust_clock` routine.
///
/// Our goal is to align `CLOCK_REALTIME` frequency with the frequency of the clock determined by `ClockBound`.
/// We use `ClockBound` clock as a frequency standard for `CLOCK_REALTIME`.
/// Let t = some instant
/// Let R(t) = `CLOCK_REALTIME` reading at t
/// Let C(t) = `ClockBound` clock estimate of UTC at t
/// Let F(t) = `CLOCK_REALTIME` frequency at t
///
/// We take snapshots of the clock which allow us to construct the following timestamps at T(old) and T(new)
/// ```text
/// R(old) = `CLOCK_REALTIME` at old
/// C(old) = `ClockBound` at old
/// R(new) = `CLOCK_REALTIME` at new
/// C(new) = `ClockBound` at new
/// F(old) = `CLOCK_REALTIME` frequency correction at old
/// F(new) is not used, since we are calculating based on the interval between `old` and `new`
/// ```
///
/// To have the frequency of `CLOCK_REALTIME` match up with `ClockBound`, we calculate the relative frequency correction
/// w.r.t `F(old)` that would have `R(new) - R(old)` == `C(new) - C(old)`, and return that value.
///
/// Example 1:
/// ```text
/// C(old) = 0.00, C(new) = 1.00
/// R(old) = 0.00, R(new) = 1.00, F(old) = +10ppm = 1.000_010
/// Frequency to set = F(old) * (C(new) - C(old)) / ((R(new) - R(old))
/// Frequency to set = 1.000_010 * (1.00 - 0.00) / (1.00 - 0.00) = 1.000_010 (no change since clock rates are aligned)
/// ```
/// Example 2:
/// ```text
/// C(old) = 0.00, C(new) = 1.01
/// R(old) = 0.00, R(new) = 1.00, F(old) = +10ppm = 1.000_010
/// Frequency to set = 1.000_010 * (1.01 - 0.00) / (1.00 - 0.00) = 1.000_010
/// Frequency to set = 1.000_010 * 1.01 = ~1.010_010 (+1% since `ClockBound` was 1% faster over that interval)
/// ```
///
/// Note - this calculation is naively based on a linear interpolation of the two snapshots. If that relationship is non-linear,
/// it may not be perfectly precise. Sources of non-linearity include:
///  * Phase correction due to an ongoing slew (e.g. `offset` supplied to the kernel)
///     * This may be possible to mitigate if we are able to calculate this phase correction and adjust our `R(new) - R(old)` calculation based on it
///  * Any frequency change between the two snapshots (if another entity modifies `freq` in kernel while `ClockBound` is running)
///
/// We at least partially mitigate the `PhaseCorrection` by reading the change in offset between snapshots, and modifying our `CLOCK_REALTIME`
/// interval by that amount.
/// But if another frequency or phase correction occurred between these two snapshots (another time service or operator?), all bets are off.
#[allow(
    clippy::cast_precision_loss,
    reason = "diff in two snapshot TSC values should not be susceptible to significant enough loss of precision to hurt us"
)]
fn calculate_frequency_correction(
    clock_params: &ClockParameters,
    old_snapshot: &ClockSnapshot,
    new_snapshot: &ClockSnapshot,
) -> Skew {
    let SystemClockMeasurement {
        tsc: old_tsc_value,
        system_time: old_realtime_ts,
    } = old_snapshot.system_clock;
    let SystemClockMeasurement {
        tsc: new_tsc_value,
        system_time: new_realtime_ts,
    } = new_snapshot.system_clock;

    let old_freq = old_snapshot.kernel_state.freq();
    let diff_clock_realtime = new_realtime_ts - old_realtime_ts;
    let diff_clockbound_seconds =
        (new_tsc_value.get() - old_tsc_value.get()) as f64 * clock_params.period.get();
    let clockbound_rate_wrt_clock_realtime =
        diff_clockbound_seconds / diff_clock_realtime.as_seconds_f64();

    let old_frequency_clock_realtime = old_freq.get() + 1.0;
    let fractional_correction =
        (clockbound_rate_wrt_clock_realtime * old_frequency_clock_realtime) - 1.0;
    Skew::from_ppm(fractional_correction * 1e6)
}

/// Snapshot of `CLOCK_REALTIME` at some point in time, including the frequency correction
/// via `ntp_adjtime` read-only call and the TSC value aligned with the approximate `CLOCK_REALTIME`
/// timestamp (only aligned via interleaved reads, and the TSC value is an approximation by getting the midpoint of those reads).
///
/// Notably, the `clock_realtime_frequency_correction` may not be reliable if any other service or operator
/// modifies the `freq` value in kernel...
#[derive(Debug, PartialEq, Clone)]
pub(super) struct ClockSnapshot {
    pub(super) system_clock: SystemClockMeasurement,
    pub(super) kernel_state: Timex,
}
impl ClockSnapshot {
    pub fn retrieve(ntp_adjtime: &impl NtpAdjTimeExt) -> Self {
        // Unwrap safety: retrieving adjtime parameters should succeed regardless of
        // timex status. If we received an actual error from the system call, we have
        // nothing better we can do here
        let kernel_state = ntp_adjtime
            .read_adjtime()
            .expect("failed to retrieve ntp_adjtime parameters from kernel");
        let system_clock = SystemClockMeasurement::now();
        Self {
            system_clock,
            kernel_state,
        }
    }
}

#[cfg(test)]
mod test {
    use rstest::rstest;

    use crate::daemon::{
        clock_state::clock_adjust::{NtpAdjTimeError, ntp_adjtime::MockNtpAdjTimeExt},
        time::{
            Instant, TscCount,
            tsc::{Frequency, Period},
        },
    };

    use super::*;

    fn test_clock_parameters() -> ClockParameters {
        ClockParameters {
            tsc_count: TscCount::new(0),
            period: Period::from_frequency(Frequency::from_hz(1_000_000.0)),
            time: Instant::new(0),
            clock_error_bound: Duration::new(0),
            period_max_error: Period::from_seconds(0.0),
        }
    }

    fn test_clock_snapshot() -> ClockSnapshot {
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                system_time: Instant::from_secs(0),
                tsc: TscCount::new(0),
            },
            kernel_state: Timex::retrieve(),
        }
    }

    #[test]
    fn disrupted_to_initial_phase_correct_halted() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_apply_phase_correction()
            .once()
            .return_once(move |_: Duration| Ok(Timex::retrieve()));
        let disrupted = Disrupted;
        let _ = disrupted.to_initial_phase_correct_halted(&mock_ntp_adjtime);
    }

    #[test]
    #[should_panic(expected = "failed to halt phase correction: BadState(1)")]
    fn disrupted_to_initial_phase_correct_halted_panic_on_failed_adjtime() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_apply_phase_correction()
            .once()
            .return_once(move |_: Duration| Err(NtpAdjTimeError::BadState(1)));
        let disrupted = Disrupted;
        let _ = disrupted.to_initial_phase_correct_halted(&mock_ntp_adjtime);
    }

    #[test]
    fn initialized_to_initial_phase_correct_halted() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_apply_phase_correction()
            .once()
            .return_once(move |_: Duration| Ok(Timex::retrieve()));
        mock_ntp_adjtime
            .expect_step_clock()
            .once()
            .return_once(move |_: Duration| Ok(Timex::retrieve()));
        let initialized = Initialized;
        let _ = initialized
            .to_initial_phase_correct_halted(&mock_ntp_adjtime, &test_clock_parameters());
    }

    #[test]
    #[should_panic(expected = "failed to halt phase correction: BadState(1)")]
    fn initialized_to_initial_phase_correct_halted_panic_on_phase_correct_halt_failure() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_apply_phase_correction()
            .once()
            .return_once(move |_: Duration| Err(NtpAdjTimeError::BadState(1)));
        mock_ntp_adjtime
            .expect_step_clock()
            .never()
            .return_once(move |_: Duration| Ok(Timex::retrieve()));
        let initialized = Initialized;
        let _ = initialized
            .to_initial_phase_correct_halted(&mock_ntp_adjtime, &test_clock_parameters());
    }

    #[test]
    #[should_panic(expected = "failed to step clock: BadState(1)")]
    fn initialized_to_initial_phase_correct_halted_panic_on_step_clock_failure() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_apply_phase_correction()
            .once()
            .return_once(move |_: Duration| Ok(Timex::retrieve()));
        mock_ntp_adjtime
            .expect_step_clock()
            .once()
            .return_once(move |_: Duration| Err(NtpAdjTimeError::BadState(1)));
        let initialized = Initialized;
        let _ = initialized
            .to_initial_phase_correct_halted(&mock_ntp_adjtime, &test_clock_parameters());
    }

    #[test]
    fn initial_phase_correct_halted_to_initial_snapshot_retrieved() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_read_adjtime()
            .once()
            .return_once(move || Ok(Timex::retrieve()));
        let initial_phase_correct_halted = InitialPhaseCorrectHalted {
            instant: tokio::time::Instant::now(),
        };
        let _ = initial_phase_correct_halted.to_initial_snapshot_retrieved(&mock_ntp_adjtime);
    }

    #[test]
    #[should_panic(expected = "failed to retrieve ntp_adjtime parameters from kernel: BadState(1)")]
    fn initial_phase_correct_halted_to_initial_snapshot_retrieved_panic_on_fail_adjtime() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_read_adjtime()
            .once()
            .return_once(move || Err(NtpAdjTimeError::BadState(1)));
        let initial_phase_correct_halted = InitialPhaseCorrectHalted {
            instant: tokio::time::Instant::now(),
        };
        let _ = initial_phase_correct_halted.to_initial_snapshot_retrieved(&mock_ntp_adjtime);
    }

    #[test]
    fn initial_snapshot_retrieved_to_clock_adjusted() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_read_adjtime()
            .once()
            .return_once(move || Ok(Timex::retrieve()));
        mock_ntp_adjtime
            .expect_adjust_clock()
            .once()
            .return_once(move |_: Duration, _: Skew| Ok(Timex::retrieve()));
        let initial_snapshot_retrieved = InitialSnapshotRetrieved {
            instant: tokio::time::Instant::now(),
            snapshot: test_clock_snapshot(),
        };
        let _ = initial_snapshot_retrieved.to_clock_adjusted(
            &mock_ntp_adjtime,
            &test_clock_parameters(),
            &test_clock_snapshot(),
        );
    }

    #[test]
    #[should_panic(expected = "failed to retrieve ntp_adjtime parameters from kernel: BadState(1)")]
    fn initial_snapshot_retrieved_to_clock_adjusted_panic_on_failed_retrieve() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_read_adjtime()
            .once()
            .return_once(move || Err(NtpAdjTimeError::BadState(1)));
        mock_ntp_adjtime
            .expect_adjust_clock()
            .never()
            .return_once(move |_: Duration, _: Skew| Ok(Timex::retrieve()));
        let initial_snapshot_retrieved = InitialSnapshotRetrieved {
            instant: tokio::time::Instant::now(),
            snapshot: test_clock_snapshot(),
        };
        let _ = initial_snapshot_retrieved.to_clock_adjusted(
            &mock_ntp_adjtime,
            &test_clock_parameters(),
            &test_clock_snapshot(),
        );
    }

    #[test]
    #[should_panic(expected = "failed to adjust clock frequency and phase correction: BadState(1)")]
    fn initial_snapshot_retrieved_to_clock_adjusted_panic_on_failed_adjustment() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_read_adjtime()
            .once()
            .return_once(move || Ok(Timex::retrieve()));
        mock_ntp_adjtime
            .expect_adjust_clock()
            .once()
            .return_once(move |_: Duration, _: Skew| Err(NtpAdjTimeError::BadState(1)));
        let initial_snapshot_retrieved = InitialSnapshotRetrieved {
            instant: tokio::time::Instant::now(),
            snapshot: test_clock_snapshot(),
        };
        let _ = initial_snapshot_retrieved.to_clock_adjusted(
            &mock_ntp_adjtime,
            &test_clock_parameters(),
            &test_clock_snapshot(),
        );
    }

    #[test]
    fn clock_adjusted_to_phase_correct_halted() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_apply_phase_correction()
            .once()
            .return_once(move |_: Duration| Ok(Timex::retrieve()));
        let clock_adjusted = ClockAdjusted {
            instant: tokio::time::Instant::now(),
        };
        let _ = clock_adjusted.to_phase_correct_halted(&mock_ntp_adjtime);
    }

    #[test]
    #[should_panic(expected = "failed to halt phase correction: BadState(1)")]
    fn clock_adjusted_to_phase_correct_halted_panic_on_failed_phase_correct_halt() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_apply_phase_correction()
            .once()
            .return_once(move |_: Duration| Err(NtpAdjTimeError::BadState(1)));
        let clock_adjusted = ClockAdjusted {
            instant: tokio::time::Instant::now(),
        };
        let _ = clock_adjusted.to_phase_correct_halted(&mock_ntp_adjtime);
    }

    #[test]
    fn phase_correct_halted_to_snapshot_retrieved() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_read_adjtime()
            .once()
            .return_once(move || Ok(Timex::retrieve()));
        let phase_correct_halted = PhaseCorrectHalted {
            instant: tokio::time::Instant::now(),
        };
        let _ = phase_correct_halted.to_snapshot_retrieved(&mock_ntp_adjtime);
    }

    #[test]
    #[should_panic(expected = "failed to retrieve ntp_adjtime parameters from kernel: BadState(1)")]
    fn phase_correct_halted_to_snapshot_retrieved_panic_on_failed_retrieve() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_read_adjtime()
            .once()
            .return_once(move || Err(NtpAdjTimeError::BadState(1)));
        let phase_correct_halted = PhaseCorrectHalted {
            instant: tokio::time::Instant::now(),
        };
        let _ = phase_correct_halted.to_snapshot_retrieved(&mock_ntp_adjtime);
    }

    #[test]
    fn snapshot_retrieved_to_clock_adjusted() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_read_adjtime()
            .once()
            .return_once(move || Ok(Timex::retrieve()));
        mock_ntp_adjtime
            .expect_adjust_clock()
            .once()
            .return_once(move |_: Duration, _: Skew| Ok(Timex::retrieve()));
        let snapshot_retrieved = SnapshotRetrieved {
            instant: tokio::time::Instant::now(),
            snapshot: test_clock_snapshot(),
        };
        let _ = snapshot_retrieved.to_clock_adjusted(
            &mock_ntp_adjtime,
            &test_clock_parameters(),
            &test_clock_snapshot(),
        );
    }

    #[test]
    #[should_panic(expected = "failed to retrieve ntp_adjtime parameters from kernel: BadState(1)")]
    fn snapshot_retrieved_to_clock_adjusted_panic_on_failed_retrieve() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_read_adjtime()
            .once()
            .return_once(move || Err(NtpAdjTimeError::BadState(1)));
        mock_ntp_adjtime
            .expect_adjust_clock()
            .never()
            .return_once(move |_: Duration, _: Skew| Ok(Timex::retrieve()));
        let snapshot_retrieved = SnapshotRetrieved {
            instant: tokio::time::Instant::now(),
            snapshot: test_clock_snapshot(),
        };
        let _ = snapshot_retrieved.to_clock_adjusted(
            &mock_ntp_adjtime,
            &test_clock_parameters(),
            &test_clock_snapshot(),
        );
    }

    #[test]
    #[should_panic(expected = "failed to adjust clock frequency and phase correction: BadState(1)")]
    fn snapshot_retrieved_to_clock_adjusted_panic_on_failed_adjustment() {
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_read_adjtime()
            .once()
            .return_once(move || Ok(Timex::retrieve()));
        mock_ntp_adjtime
            .expect_adjust_clock()
            .once()
            .return_once(move |_: Duration, _: Skew| Err(NtpAdjTimeError::BadState(1)));
        let snapshot_retrieved = SnapshotRetrieved {
            instant: tokio::time::Instant::now(),
            snapshot: test_clock_snapshot(),
        };
        let _ = snapshot_retrieved.to_clock_adjusted(
            &mock_ntp_adjtime,
            &test_clock_parameters(),
            &test_clock_snapshot(),
        );
    }

    #[rstest]
    #[case::clocks_aligned(
        ClockParameters {
            tsc_count: TscCount::new(0),
            period: Period::from_frequency(Frequency::from_hz(1.0)),
            time: Instant::new(0),
            clock_error_bound: Duration::new(0),
            period_max_error: Period::from_seconds(0.0),
        },
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                tsc: TscCount::new(0),
                system_time: Instant::from_secs(0),
            },
            kernel_state: Timex::frequency_correction(Skew::from_ppm(0.0)),
        },
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                tsc: TscCount::new(1),
                system_time: Instant::from_secs(1),
            },
            kernel_state: Timex::frequency_correction(Skew::from_ppm(0.0)), // not used
        },
        Skew::from_ppm(0.0),
    )]
    #[case::clockbound_1_ppm_slow_of_clock_realtime_with_initial_freq_100_ppm(
        // 1.000_100 * 0.999_999 = 1.000_098_999_900
        ClockParameters {
            tsc_count: TscCount::new(0),
            period: Frequency::from_hz(1_000_000.0).period(),
            time: Instant::new(0),
            clock_error_bound: Duration::new(0),
            period_max_error: Period::from_seconds(0.0),
        },
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                tsc: TscCount::new(0),
                system_time: Instant::from_secs(0),
            },
            kernel_state: Timex::frequency_correction(Skew::from_ppm(100.0)),
        },
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                tsc: TscCount::new(999_999),
                system_time: Instant::from_secs(1),
            },
            kernel_state: Timex::frequency_correction(Skew::from_ppm(0.0)), // not used
        },
        Skew::from_ppm(98.999_900),
    )]
    #[case::clockbound_1_ppm_fast_of_clock_realtime_with_initial_freq_100_ppm(
        // 1.000_100 * 1.000_001 = 1.000_101_000_100
        ClockParameters {
            tsc_count: TscCount::new(0),
            period: Frequency::from_hz(1_000_000.0).period(),
            time: Instant::new(0),
            clock_error_bound: Duration::new(0),
            period_max_error: Period::from_seconds(0.0),
        },
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                tsc: TscCount::new(0),
                system_time: Instant::from_secs(0),
            },
            kernel_state: Timex::frequency_correction(Skew::from_ppm(100.0)),
        },
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                tsc: TscCount::new(1_000_001),
                system_time: Instant::from_secs(1),
            },
            kernel_state: Timex::frequency_correction(Skew::from_ppm(0.0)), // not used
        },
        Skew::from_ppm(101.000_100),
    )]
    #[case::clockbound_2_point_5_ppm_fast_of_clock_realtime_with_initial_freq_100_ppm(
        // 1.000_100 * 1.000_002_500 = 1.000_102_500_250
        ClockParameters {
            tsc_count: TscCount::new(0),
            period: Frequency::from_hz(1_000_000.0).period(),
            time: Instant::new(0),
            clock_error_bound: Duration::new(0),
            period_max_error: Period::from_seconds(0.0),
        },
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                tsc: TscCount::new(0),
                system_time: Instant::from_secs(0),
            },
            kernel_state: Timex::frequency_correction(Skew::from_ppm(100.0)),
        },
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                tsc: TscCount::new(2_000_005),
                system_time: Instant::from_secs(2),
            },
            kernel_state: Timex::frequency_correction(Skew::from_ppm(0.0)), // not used
        },
        Skew::from_ppm(102.50025),
    )]
    #[case::clockbound_2_point_5_ppm_slow_of_clock_realtime_with_initial_freq_100_ppm(
        // 1.000_100 * 0.999_997_500 = 1.000_097_499_750
        ClockParameters {
            tsc_count: TscCount::new(0),
            period: Frequency::from_hz(1_000_000.0).period(),
            time: Instant::new(0),
            clock_error_bound: Duration::new(0),
            period_max_error: Period::from_seconds(0.0),
        },
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                tsc: TscCount::new(0),
                system_time: Instant::from_secs(0),
            },
            kernel_state: Timex::frequency_correction(Skew::from_ppm(100.0)),
        },
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                tsc: TscCount::new(1_999_995),
                system_time: Instant::from_secs(2),
            },
            kernel_state: Timex::frequency_correction(Skew::from_ppm(0.0)), // not used
        },
        Skew::from_ppm(97.499_750),
    )]
    #[case::clockbound_2_point_5_ppm_fast_of_clock_realtime_with_initial_freq_negative_100_ppm(
        // 0.999_900 * 1.000_002_500 = 0.999_902_499
        // -(1 - 0.999_902_499) = -0.000_097_500_250
        ClockParameters {
            tsc_count: TscCount::new(0),
            period: Frequency::from_hz(1_000_000.0).period(),
            time: Instant::new(0),
            clock_error_bound: Duration::new(0),
            period_max_error: Period::from_seconds(0.0),
        },
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                tsc: TscCount::new(0),
                system_time: Instant::from_secs(0),
            },
            kernel_state: Timex::frequency_correction(-Skew::from_ppm(100.0)),
        },
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                tsc: TscCount::new(2_000_005),
                system_time: Instant::from_secs(2),
            },
            kernel_state: Timex::frequency_correction(Skew::from_ppm(0.0)), // not used
        },
        -Skew::from_ppm(97.500_250),
    )]
    #[case::clockbound_2_point_5_ppm_slow_of_clock_realtime_with_initial_freq_negative_100_ppm(
        // 0.999_900 * 0.999_997_500 = 0.999_897_500_250
        // -(1 - 0.999_897_500_250) = -0.000_102_499_749
        ClockParameters {
            tsc_count: TscCount::new(0),
            period: Frequency::from_hz(1_000_000.0).period(),
            time: Instant::new(0),
            clock_error_bound: Duration::new(0),
            period_max_error: Period::from_seconds(0.0),
        },
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                tsc: TscCount::new(0),
                system_time: Instant::from_secs(0),
            },
            kernel_state: Timex::frequency_correction(-Skew::from_ppm(100.0)),
        },
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                tsc: TscCount::new(1_999_995),
                system_time: Instant::from_secs(2),
            },
            kernel_state: Timex::frequency_correction(Skew::from_ppm(0.0)), // not used
        },
        -Skew::from_ppm(102.499_749_999_984_680),
    )]
    fn test_calculate_new_frequency_correction(
        #[case] clock_params: ClockParameters,
        #[case] old_snapshot: ClockSnapshot,
        #[case] new_snapshot: ClockSnapshot,
        #[case] expected: Skew,
    ) {
        approx::assert_abs_diff_eq!(
            calculate_frequency_correction(&clock_params, &old_snapshot, &new_snapshot).get(),
            expected.get()
        );
    }
}
