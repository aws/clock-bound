//! Finite State Machine implementation of the clock status written to the SHM segment.
//!
//! The implementation leverages zero-sized types to represent the various states of the FSM.
//! Each state tracks the last clock status retrieved from chronyd as well as the last clock
//! disruption status.
//! The transitions between states are triggered by calling the `apply_chrony()` and
//! `apply_disruption()` to the current state. Pattern matching is used to make sure all
//! combinations of ChronyClockStatus and ClockDisruptionState are covered.

use tracing::debug;

use clock_bound_shm::ClockStatus;

use crate::ChronyClockStatus;
use crate::ClockDisruptionState;

/// Internal trait to model a FSM transition.
///
/// This trait is a bound on FSMState, which is the public interface. This means this FSMTransition
/// trait has to be marked public too.
/// An alternative implementation would be to have `transition()` be part of FSMState. This would
/// expose `transition()` to the caller, as a function available on types implementing FSMState.
/// Having this internal trait let us write a blanket implementation of the FSMTransition trait.
pub trait FSMTransition {
    /// The execution of the FSM is a transition from one state to another.
    ///
    /// Applying `transition()` on a state returns the next state. The FSM is a graph, and the
    /// input to `transition()` conditions which state is returned. The current implementation
    /// leverages marker types: every state is a different type. Hence the return type of
    /// `transition()` is "something that implements FSMState". Because `transition()` may return
    /// more than one type, the trait has to be Box'ed in.
    ///
    /// Note that `transition()` returns a <dyn FSMState> (and not a FSMTransition trait!). This
    /// hides the internal detail for the caller using this FSM>
    fn transition(
        &self,
        chrony: ChronyClockStatus,
        disruption: ClockDisruptionState,
    ) -> Box<dyn FSMState>;
}

/// External trait to execute the FSM that drives the clock status value in the shared memory segment.
///
/// Note that the FSMState trait is bound by the FSMTransition trait. This decoupling allow for a
/// blanket implementation of the trait for all the FSM states, while enforcing an implementation
/// pattern where the FSM logic is to be implemented in the FSMTransition trait.
pub trait FSMState: FSMTransition {
    /// Apply a new chrony clock status to the FSM, possibly changing the current state.
    fn apply_chrony(&self, update: ChronyClockStatus) -> Box<dyn FSMState>;

    /// Apply a new clock disruption event to the FSM, possibly changing the current state.
    fn apply_disruption(&self, update: ClockDisruptionState) -> Box<dyn FSMState>;

    /// Return the value of the current FSM state, a clock status to write to the SHM segment.
    fn value(&self) -> ClockStatus;
}

/// Define the possible states of the FSM that drives the clock status written to the SHM segment.
///
/// These zero-sized unit struct parameterize the more generic ShmClockState<T> struct.
pub struct Unknown;
pub struct Synchronized;
pub struct FreeRunning;
pub struct Disrupted;

/// The state the FSM is currently in.
///
/// Note the default type parameter is `Unknown`, the expected initial state for the FSM.
pub struct ShmClockState<State = Unknown> {
    // Marker type eliminated at compile time
    _state: std::marker::PhantomData<State>,

    // The status of the clock retrieved from chronyd that led to entering this state.
    chrony: ChronyClockStatus,

    // The clock disruption event that led to entering this state.
    disruption: ClockDisruptionState,

    // The value of the state, determined from the combination of chrony and disruption values.
    clock_status: ClockStatus,
}

/// Implement Default trait for ShmClockState.
///
/// The type parameter is left out in this impl block, as it defaults to `Unknown` and hides the
/// internals of the FSM away for the caller, while guiding all instantiations to start in the
/// `Unknown` state.
impl Default for ShmClockState {
    /// Create a new state, effectively a new FSM whose execution starts at `Unknown`
    ///
    // The FSM starts with no assumption on the state of the clock.
    fn default() -> Self {
        ShmClockState::<Unknown> {
            _state: std::marker::PhantomData::<Unknown>,
            chrony: ChronyClockStatus::Unknown,
            disruption: ClockDisruptionState::Unknown,
            clock_status: ClockStatus::Unknown,
        }
    }
}

/// Macro to generate generic impl block for the ShmClockState with corresponding type parameter.
///
/// `new()` needs to store the specific clock_status on the new state, which we cannot easily use a
/// blanket implementation for. So this macro is the next best thing to avoid repetitive blocks of
/// code. Note that `new()` is kept private. `default()` should be the only mechanism for the
/// caller to instantiate a FSM.
macro_rules! shm_clock_state_impl {
    ($state:ty, $state_clock:expr) => {
        impl ShmClockState<$state> {
            fn new(chrony: ChronyClockStatus, disruption: ClockDisruptionState) -> Self {
                ShmClockState {
                    _state: std::marker::PhantomData::<$state>,
                    clock_status: $state_clock,
                    chrony,
                    disruption,
                }
            }
        }
    };
}

// Generate impl block for all ShmClockState<T>
shm_clock_state_impl!(Unknown, ClockStatus::Unknown);
shm_clock_state_impl!(Synchronized, ClockStatus::Synchronized);
shm_clock_state_impl!(FreeRunning, ClockStatus::FreeRunning);
shm_clock_state_impl!(Disrupted, ClockStatus::Disrupted);

/// Blanket implementation of external FSMState trait for all ShmClockState<T>
impl<T> FSMState for ShmClockState<T>
where
    ShmClockState<T>: FSMTransition,
{
    /// Return the clock status for this FSM state.
    fn value(&self) -> ClockStatus {
        self.clock_status
    }

    /// Apply a new chronyd ChronyClockStatus to the FSM
    fn apply_chrony(&self, update: ChronyClockStatus) -> Box<dyn FSMState> {
        debug!("Before applying new ChronyClockStatus {:?}, self.chrony is: {:?}, self.disruption is: {:?}, self.value() is: {:?}",
              update, self.chrony, self.disruption, self.value());
        let rv = self.transition(update, self.disruption);
        debug!(
            "After  applying new ChronyClockStatus {:?}, rv.value() is: {:?}",
            update,
            rv.value()
        );
        rv
    }

    /// Apply a new ClockDisruptionState to the FSM
    fn apply_disruption(&self, update: ClockDisruptionState) -> Box<dyn FSMState> {
        debug!("Before applying new ClockDisruptionState {:?}, self.chrony is: {:?}, self.disruption is: {:?}, self.value() is: {:?}",
              update, self.chrony, self.disruption, self.value());
        let rv = self.transition(self.chrony, update);
        debug!(
            "After  applying new ClockDisruptionState {:?}, rv.value() is: {:?}",
            update,
            rv.value()
        );
        rv
    }
}

/// Macro to create a boxed ShmClockState from a type parameter, chrony and disruption status.
///
/// This macro makes the implementation of `transition()` cases easier to read and reason
/// about.
macro_rules! bstate {
    ($state:ty, $chrony:expr, $disruption:expr) => {
        Box::new(ShmClockState::<$state>::new($chrony, $disruption))
    };
}

impl FSMTransition for ShmClockState<Unknown> {
    /// Implement the transitions from the Unknown FSM state.
    fn transition(
        &self,
        chrony: ChronyClockStatus,
        disruption: ClockDisruptionState,
    ) -> Box<dyn FSMState> {
        // Match on all parameters, the compiler will make sure no combination is missed. Some
        // combinations are elided, remember the first matching arm wins.
        match (chrony, disruption) {
            (ChronyClockStatus::Synchronized, ClockDisruptionState::Reliable) => {
                bstate!(Synchronized, chrony, disruption)
            }
            (ChronyClockStatus::FreeRunning, ClockDisruptionState::Reliable) => {
                bstate!(Unknown, chrony, disruption)
            }
            (_, ClockDisruptionState::Disrupted) => bstate!(Disrupted, chrony, disruption),
            (ChronyClockStatus::Unknown, _) => bstate!(Unknown, chrony, disruption),
            (_, ClockDisruptionState::Unknown) => bstate!(Unknown, chrony, disruption),
        }
    }
}

impl FSMTransition for ShmClockState<Synchronized> {
    /// Implement the transitions from the Synchronized FSM state.
    fn transition(
        &self,
        chrony: ChronyClockStatus,
        disruption: ClockDisruptionState,
    ) -> Box<dyn FSMState> {
        // Match on all parameters, the compiler will make sure no combination is missed. Some
        // combinations are elided, remember the first matching arm wins.
        match (chrony, disruption) {
            (ChronyClockStatus::Synchronized, ClockDisruptionState::Reliable) => {
                bstate!(Synchronized, chrony, disruption)
            }
            (ChronyClockStatus::FreeRunning, ClockDisruptionState::Reliable) => {
                bstate!(FreeRunning, chrony, disruption)
            }
            (_, ClockDisruptionState::Disrupted) => bstate!(Disrupted, chrony, disruption),
            (ChronyClockStatus::Unknown, _) => bstate!(Unknown, chrony, disruption),
            (_, ClockDisruptionState::Unknown) => bstate!(Unknown, chrony, disruption),
        }
    }
}

impl FSMTransition for ShmClockState<FreeRunning> {
    /// Implement the transitions from the FreeRunning FSM state.
    fn transition(
        &self,
        chrony: ChronyClockStatus,
        disruption: ClockDisruptionState,
    ) -> Box<dyn FSMState> {
        // Match on all parameters, the compiler will make sure no combination is missed. Some
        // combinations are elided, remember the first matching arm wins.
        match (chrony, disruption) {
            (ChronyClockStatus::Synchronized, ClockDisruptionState::Reliable) => {
                bstate!(Synchronized, chrony, disruption)
            }
            (ChronyClockStatus::FreeRunning, ClockDisruptionState::Reliable) => {
                bstate!(FreeRunning, chrony, disruption)
            }
            (_, ClockDisruptionState::Disrupted) => bstate!(Disrupted, chrony, disruption),
            (ChronyClockStatus::Unknown, _) => bstate!(Unknown, chrony, disruption),
            (_, ClockDisruptionState::Unknown) => bstate!(Unknown, chrony, disruption),
        }
    }
}

impl FSMTransition for ShmClockState<Disrupted> {
    /// Implement the transitions from the Disrupted FSM state.
    fn transition(
        &self,
        chrony: ChronyClockStatus,
        disruption: ClockDisruptionState,
    ) -> Box<dyn FSMState> {
        // Match on all parameters, the compiler will make sure no combination is missed. Some
        // combinations are elided, remember the first matching arm wins.
        match (chrony, disruption) {
            (_, ClockDisruptionState::Disrupted) => bstate!(Disrupted, chrony, disruption),
            (_, ClockDisruptionState::Unknown) => bstate!(Unknown, chrony, disruption),
            (_, ClockDisruptionState::Reliable) => {
                bstate!(Unknown, chrony, disruption)
            }
        }
    }
}

#[cfg(test)]
mod t_clock_state_fsm {

    use super::*;

    fn _helper_generate_chrony_status() -> Vec<ChronyClockStatus> {
        vec![
            ChronyClockStatus::Unknown,
            ChronyClockStatus::Synchronized,
            ChronyClockStatus::FreeRunning,
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
        let state = ShmClockState::default();
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

        let state = bstate!(
            Disrupted,
            ChronyClockStatus::Synchronized,
            ClockDisruptionState::Disrupted
        );
        assert_eq!(state.value(), ClockStatus::Disrupted);
    }

    /// Assert that unknown input from Unknown leads to the unknown state.
    #[test]
    fn test_transition_with_unknown_from_unknown() {
        for status in _helper_generate_chrony_status() {
            let state = bstate!(
                Unknown,
                ChronyClockStatus::Unknown,
                ClockDisruptionState::Unknown
            );
            let state = state.transition(status, ClockDisruptionState::Unknown);
            assert_eq!(state.value(), ClockStatus::Unknown);
        }

        for status in _helper_generate_disruption_status() {
            let state = bstate!(
                Unknown,
                ChronyClockStatus::Unknown,
                ClockDisruptionState::Unknown
            );
            let state = state.transition(ChronyClockStatus::Unknown, status);
            if status == ClockDisruptionState::Disrupted {
                assert_eq!(state.value(), ClockStatus::Disrupted);
            } else {
                assert_eq!(state.value(), ClockStatus::Unknown);
            }
        }
    }

    /// Assert that unknown input from Synchronized leads to the Unknown state.
    #[test]
    fn test_transition_with_unknown_from_synchronized() {
        for status in _helper_generate_chrony_status() {
            let state = bstate!(
                Synchronized,
                ChronyClockStatus::Synchronized,
                ClockDisruptionState::Reliable
            );
            let state = state.transition(status, ClockDisruptionState::Unknown);
            assert_eq!(state.value(), ClockStatus::Unknown);
        }

        for status in _helper_generate_disruption_status() {
            let state = bstate!(
                Synchronized,
                ChronyClockStatus::Synchronized,
                ClockDisruptionState::Reliable
            );
            let state = state.transition(ChronyClockStatus::Unknown, status);
            if status == ClockDisruptionState::Disrupted {
                assert_eq!(state.value(), ClockStatus::Disrupted);
            } else {
                assert_eq!(state.value(), ClockStatus::Unknown);
            }
        }
    }

    /// Assert that unknown input from FreeRunning leads to the Unknown state.
    #[test]
    fn test_transition_with_unknown_from_freerunning() {
        for status in _helper_generate_chrony_status() {
            let state = bstate!(
                FreeRunning,
                ChronyClockStatus::FreeRunning,
                ClockDisruptionState::Reliable
            );
            let state = state.transition(status, ClockDisruptionState::Unknown);
            assert_eq!(state.value(), ClockStatus::Unknown);
        }

        for status in _helper_generate_disruption_status() {
            let state = bstate!(
                FreeRunning,
                ChronyClockStatus::FreeRunning,
                ClockDisruptionState::Reliable
            );
            let state = state.transition(ChronyClockStatus::Unknown, status);
            if status == ClockDisruptionState::Disrupted {
                assert_eq!(state.value(), ClockStatus::Disrupted);
            } else {
                assert_eq!(state.value(), ClockStatus::Unknown);
            }
        }
    }

    /// Assert that unknown input from Disrupted does NOT transition to Unknown state, except if
    /// the clock is reliable
    #[test]
    fn test_transition_with_unknown_from_disrupted() {
        for status in _helper_generate_chrony_status() {
            let state = bstate!(
                Disrupted,
                ChronyClockStatus::Synchronized,
                ClockDisruptionState::Disrupted
            );
            let state = state.transition(status, ClockDisruptionState::Unknown);
            assert_eq!(state.value(), ClockStatus::Unknown);
        }

        for status in _helper_generate_disruption_status() {
            let state = bstate!(
                Disrupted,
                ChronyClockStatus::Synchronized,
                ClockDisruptionState::Disrupted
            );
            let state = state.transition(ChronyClockStatus::Unknown, status);
            if status == ClockDisruptionState::Disrupted {
                assert_eq!(state.value(), ClockStatus::Disrupted);
            } else {
                assert_eq!(state.value(), ClockStatus::Unknown);
            }
        }
    }

    /// Assert that disrupted input always lead to the Disrupted state
    #[test]
    fn test_transition_into_disrupted() {
        // Synchronized -> Disrupted
        for status in _helper_generate_chrony_status() {
            let state = bstate!(
                Synchronized,
                ChronyClockStatus::Synchronized,
                ClockDisruptionState::Reliable
            );
            let state = state.transition(status, ClockDisruptionState::Disrupted);
            assert_eq!(state.value(), ClockStatus::Disrupted);
        }

        // FreeRunning -> Disrupted
        for status in _helper_generate_chrony_status() {
            let state = bstate!(
                FreeRunning,
                ChronyClockStatus::FreeRunning,
                ClockDisruptionState::Reliable
            );
            let state = state.transition(status, ClockDisruptionState::Disrupted);
            assert_eq!(state.value(), ClockStatus::Disrupted);
        }

        // Disrupted -> Disrupted
        for status in _helper_generate_chrony_status() {
            let state = bstate!(
                Disrupted,
                ChronyClockStatus::Synchronized,
                ClockDisruptionState::Disrupted
            );
            let state = state.transition(status, ClockDisruptionState::Disrupted);
            assert_eq!(state.value(), ClockStatus::Disrupted);
        }
    }

    /// Assert that disrupted state always leads to Unknown.
    #[test]
    fn test_transition_from_disrupted() {
        for status in _helper_generate_chrony_status() {
            let state = bstate!(Disrupted, status, ClockDisruptionState::Disrupted);
            let state = state.transition(status, ClockDisruptionState::Reliable);
            assert_eq!(state.value(), ClockStatus::Unknown);
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

    /// Assert that apply_disruption is functional.
    #[test]
    fn test_apply_disruption() {
        let state = bstate!(
            Synchronized,
            ChronyClockStatus::Synchronized,
            ClockDisruptionState::Reliable
        );

        let state = state.apply_disruption(ClockDisruptionState::Unknown);
        assert_eq!(state.value(), ClockStatus::Unknown);
    }
}
