//! Finite State Machine implementation of the clock status written to the SHM segment when
//! clock disruption is NOT supported.
//!
//! This implementation is a trimmed down version of the `ShmClockState<S>` that ignores all
//! clock disruption events.

use clock_bound_shm::ClockStatus;

use crate::clock_state_fsm::{FSMState, FSMTransition, FreeRunning, Synchronized, Unknown};
use crate::ChronyClockStatus;
use crate::ClockDisruptionState;

/// The state the FSM is currently in.
///
/// Note the default type parameter is `Unknown`, the expected initial state for the FSM.
pub struct ShmClockStateNoDisruption<State = Unknown> {
    // Marker type eliminated at compile time
    _state: std::marker::PhantomData<State>,

    // The status of the clock retrieved from chronyd that led to entering this state.
    chrony: ChronyClockStatus,

    // The clock disruption event that led to entering this state.
    disruption: ClockDisruptionState,

    // The value of the state, determined from the combination of chrony and disruption values.
    clock_status: ClockStatus,
}

/// Implement Default trait for ShmClockStateNoDisruption.
///
/// The type parameter is left out in this impl block, as it defaults to `Unknown` and hides the
/// internals of the FSM away for the caller, while guiding all instantiations to start in the
/// `Unknown` state.
impl Default for ShmClockStateNoDisruption {
    /// Create a new state, effectively a new FSM whose execution starts at `Unknown`
    ///
    // The FSM starts with no assumption on the state of the clock.
    fn default() -> Self {
        ShmClockStateNoDisruption::<Unknown> {
            _state: std::marker::PhantomData::<Unknown>,
            chrony: ChronyClockStatus::Unknown,
            disruption: ClockDisruptionState::Unknown,
            clock_status: ClockStatus::Unknown,
        }
    }
}

/// Macro to generate generic impl block for the ShmClockStateNoDisruption with corresponding type parameter.
///
/// `new()` needs to store the specific clock_status on the new state, which we cannot easily use a
/// blanket implementation for. So this macro is the next best thing to avoid repetitive blocks of
/// code. Note that `new()` is kept private. `default()` should be the only mechanism for the
/// caller to instantiate a FSM.
macro_rules! shm_clock_state_no_lm_impl {
    ($state:ty, $state_clock:expr) => {
        impl ShmClockStateNoDisruption<$state> {
            fn new(chrony: ChronyClockStatus, disruption: ClockDisruptionState) -> Self {
                ShmClockStateNoDisruption {
                    _state: std::marker::PhantomData::<$state>,
                    clock_status: $state_clock,
                    chrony,
                    disruption,
                }
            }
        }
    };
}

// Generate impl block for all ShmClockStateNoDisruption<T>
shm_clock_state_no_lm_impl!(Unknown, ClockStatus::Unknown);
shm_clock_state_no_lm_impl!(Synchronized, ClockStatus::Synchronized);
shm_clock_state_no_lm_impl!(FreeRunning, ClockStatus::FreeRunning);

/// Blanket implementation of external FSMState trait for all ShmClockStateNoDisruption<T>
impl<T> FSMState for ShmClockStateNoDisruption<T>
where
    ShmClockStateNoDisruption<T>: FSMTransition,
{
    /// Return the clock status for this FSM state.
    fn value(&self) -> ClockStatus {
        self.clock_status
    }

    /// Apply a new chronyd ChronyClockStatus to the FSM
    fn apply_chrony(&self, update: ChronyClockStatus) -> Box<dyn FSMState> {
        self.transition(update, self.disruption)
    }

    /// Apply a new ClockDisruptionState to the FSM
    fn apply_disruption(&self, update: ClockDisruptionState) -> Box<dyn FSMState> {
        self.transition(self.chrony, update)
    }
}

/// Macro to create a boxed ShmClockStateNoDisruption from a type parameter, chrony and disruption status.
///
/// This macro makes the implementation of `transition()` cases easier to read and reason
/// about.
macro_rules! bstate {
    ($state:ty, $chrony:expr, $disruption:expr) => {
        Box::new(ShmClockStateNoDisruption::<$state>::new(
            $chrony,
            $disruption,
        ))
    };
}

impl FSMTransition for ShmClockStateNoDisruption<Unknown> {
    /// Implement the transitions from the FSM state Unknown.
    fn transition(
        &self,
        chrony: ChronyClockStatus,
        _disruption: ClockDisruptionState,
    ) -> Box<dyn FSMState> {
        match chrony {
            ChronyClockStatus::Synchronized => {
                bstate!(Synchronized, chrony, ClockDisruptionState::Reliable)
            }
            ChronyClockStatus::FreeRunning => {
                bstate!(Unknown, chrony, ClockDisruptionState::Reliable)
            }
            ChronyClockStatus::Unknown => bstate!(Unknown, chrony, ClockDisruptionState::Reliable),
        }
    }
}

impl FSMTransition for ShmClockStateNoDisruption<Synchronized> {
    /// Implement the transitions from the FSM state Synchronized.
    fn transition(
        &self,
        chrony: ChronyClockStatus,
        _disruption: ClockDisruptionState,
    ) -> Box<dyn FSMState> {
        match chrony {
            ChronyClockStatus::Synchronized => {
                bstate!(Synchronized, chrony, ClockDisruptionState::Reliable)
            }
            ChronyClockStatus::FreeRunning => {
                bstate!(FreeRunning, chrony, ClockDisruptionState::Reliable)
            }
            ChronyClockStatus::Unknown => bstate!(Unknown, chrony, ClockDisruptionState::Reliable),
        }
    }
}

impl FSMTransition for ShmClockStateNoDisruption<FreeRunning> {
    /// Implement the transitions from the FSM state FreeRunning.
    fn transition(
        &self,
        chrony: ChronyClockStatus,
        _disruption: ClockDisruptionState,
    ) -> Box<dyn FSMState> {
        match chrony {
            ChronyClockStatus::Synchronized => {
                bstate!(Synchronized, chrony, ClockDisruptionState::Reliable)
            }
            ChronyClockStatus::FreeRunning => {
                bstate!(FreeRunning, chrony, ClockDisruptionState::Reliable)
            }
            ChronyClockStatus::Unknown => bstate!(Unknown, chrony, ClockDisruptionState::Reliable),
        }
    }
}

#[cfg(test)]
mod t_clock_state_fsm_no_lm {

    use super::*;

    fn _helper_generate_chrony_status() -> Vec<(ChronyClockStatus, ClockStatus)> {
        vec![
            (ChronyClockStatus::Unknown, ClockStatus::Unknown),
            (ChronyClockStatus::Synchronized, ClockStatus::Synchronized),
            (ChronyClockStatus::FreeRunning, ClockStatus::FreeRunning),
        ]
    }

    fn _helper_generate_disruption_status() -> Vec<ClockDisruptionState> {
        vec![
            ClockDisruptionState::Unknown,
            ClockDisruptionState::Reliable,
            ClockDisruptionState::Disrupted,
        ]
    }

    /// Assert that creating a FSM defaults to the Unknown state.
    #[test]
    fn test_entry_point_to_fsm() {
        let state = ShmClockStateNoDisruption::default();
        assert_eq!(state.value(), ClockStatus::Unknown);
    }

    /// Assert the clock status value return by each state is correct.
    #[test]
    fn test_state_and_value() {
        let state = bstate!(
            Unknown,
            ChronyClockStatus::Unknown,
            ClockDisruptionState::Unknown
        );
        assert_eq!(state.value(), ClockStatus::Unknown);

        let state = bstate!(
            Synchronized,
            ChronyClockStatus::Synchronized,
            ClockDisruptionState::Reliable
        );
        assert_eq!(state.value(), ClockStatus::Synchronized);

        let state = bstate!(
            FreeRunning,
            ChronyClockStatus::FreeRunning,
            ClockDisruptionState::Reliable
        );
        assert_eq!(state.value(), ClockStatus::FreeRunning);
    }

    /// Assert that chrony status drives the correct clock status from Unknown
    #[test]
    fn test_transition_chrony_from_unknown() {
        for (chrony_status, clock_status) in _helper_generate_chrony_status() {
            let state = bstate!(
                Unknown,
                ChronyClockStatus::Unknown,
                ClockDisruptionState::Unknown
            );
            let state = state.transition(chrony_status, ClockDisruptionState::Unknown);
            if chrony_status == ChronyClockStatus::FreeRunning {
                assert_eq!(state.value(), ClockStatus::Unknown);
            } else {
                assert_eq!(state.value(), clock_status);
            }
        }
    }

    /// Assert that chrony status drives the correct clock status from Synchronized
    #[test]
    fn test_transition_chrony_from_synchronized() {
        for (chrony_status, clock_status) in _helper_generate_chrony_status() {
            let state = bstate!(
                Synchronized,
                ChronyClockStatus::Synchronized,
                ClockDisruptionState::Unknown
            );
            let state = state.transition(chrony_status, ClockDisruptionState::Unknown);
            assert_eq!(state.value(), clock_status);
        }
    }

    /// Assert that chrony status drives the correct clock status from Free Running
    #[test]
    fn test_transition_chrony_from_free_running() {
        for (chrony_status, clock_status) in _helper_generate_chrony_status() {
            let state = bstate!(
                FreeRunning,
                ChronyClockStatus::FreeRunning,
                ClockDisruptionState::Unknown
            );
            let state = state.transition(chrony_status, ClockDisruptionState::Unknown);
            assert_eq!(state.value(), clock_status);
        }
    }

    #[test]
    fn test_transition_ignore_disruption_from_unknown() {
        for status in _helper_generate_disruption_status() {
            let state = bstate!(
                Unknown,
                ChronyClockStatus::Unknown,
                ClockDisruptionState::Unknown
            );
            let state = state.transition(ChronyClockStatus::Unknown, status);
            assert_eq!(state.value(), ClockStatus::Unknown);
        }
    }

    /// Assert that unknown input from Synchronized leads to the Unknown state.
    #[test]
    fn test_transition_ignore_disruption_from_synchronized() {
        for status in _helper_generate_disruption_status() {
            let state = bstate!(
                Synchronized,
                ChronyClockStatus::Synchronized,
                ClockDisruptionState::Reliable
            );
            let state = state.transition(ChronyClockStatus::Synchronized, status);
            assert_eq!(state.value(), ClockStatus::Synchronized);
        }
    }

    /// Assert that unknown input from FreeRunning leads to the Unknown state.
    #[test]
    fn test_transition_ignore_disruption_freerunning() {
        for status in _helper_generate_disruption_status() {
            let state = bstate!(
                FreeRunning,
                ChronyClockStatus::FreeRunning,
                ClockDisruptionState::Reliable
            );
            let state = state.transition(ChronyClockStatus::FreeRunning, status);
            assert_eq!(state.value(), ClockStatus::FreeRunning);
        }
    }

    /// Assert that apply_chrony is functional.
    #[test]
    fn test_apply_chrony() {
        let state = bstate!(
            Synchronized,
            ChronyClockStatus::Synchronized,
            ClockDisruptionState::Reliable
        );

        let state = state.apply_chrony(ChronyClockStatus::Unknown);
        assert_eq!(state.value(), ClockStatus::Unknown);
    }

    /// Assert that apply_disruption is ignored
    #[test]
    fn test_apply_disruption() {
        let state = bstate!(
            Synchronized,
            ChronyClockStatus::Synchronized,
            ClockDisruptionState::Reliable
        );

        let state = state.apply_disruption(ClockDisruptionState::Unknown);
        assert_eq!(state.value(), ClockStatus::Synchronized);
    }
}
