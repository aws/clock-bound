// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only

//! Finite State Machine implementation of the clock status written to the SHM segment.
//!
//! The implementation leverages zero-sized types to represent the various states of the FSM.
//! Each state tracks the last clock status retrieved from chronyd.
//! The transitions between states are triggered by calling the `apply_chrony()`
//! to the current state. Pattern matching is used to make sure all
//! combinations of ChronyClockStatus are covered.

use clock_bound_shm::ClockStatus;

use crate::ChronyClockStatus;

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
    fn transition(&self, chrony: ChronyClockStatus) -> Box<dyn FSMState>;
}

/// External trait to execute the FSM that drives the clock status value in the shared memory segment.
///
/// Note that the FSMState trait is bound by the FSMTransition trait. This decoupling allow for a
/// blanket implementation of the trait for all the FSM states, while enforcing an implementation
/// pattern where the FSM logic is to be implemented in the FSMTransition trait.
pub trait FSMState: FSMTransition {
    /// Apply a new chrony clock status to the FSM, possibly changing the current state.
    fn apply_chrony(&self, update: ChronyClockStatus) -> Box<dyn FSMState>;

    /// Return the value of the current FSM state, a clock status to write to the SHM segment.
    fn value(&self) -> ClockStatus;
}

/// Define the possible states of the FSM that drives the clock status written to the SHM segment.
///
/// These zero-sized unit struct parameterize the more generic ShmClockState<T> struct.
pub struct Unknown;
pub struct Synchronized;
pub struct FreeRunning;

/// The state the FSM is currently in.
///
/// Note the default type parameter is `Unknown`, the expected initial state for the FSM.
pub struct ShmClockState<State = Unknown> {
    // Marker type eliminated at compile time
    _state: std::marker::PhantomData<State>,

    // The value of the state, determined from the chrony values.
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
    // The FSM starts with no assumption on the state of clock synchronization managed by chrony.
    fn default() -> Self {
        ShmClockState::<Unknown> {
            _state: std::marker::PhantomData::<Unknown>,
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
            fn new() -> Self {
                ShmClockState {
                    _state: std::marker::PhantomData::<$state>,
                    clock_status: $state_clock,
                }
            }
        }
    };
}

// Generate impl block for all ShmClockState<T>
shm_clock_state_impl!(Unknown, ClockStatus::Unknown);
shm_clock_state_impl!(Synchronized, ClockStatus::Synchronized);
shm_clock_state_impl!(FreeRunning, ClockStatus::FreeRunning);

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
        self.transition(update)
    }
}

/// Macro to create a boxed ShmClockState from a type parameter, chrony status.
///
/// This macro makes the implementation of `transition()` cases easier to read and reason
/// about.
macro_rules! bstate {
    ($state:ty) => {
        Box::new(ShmClockState::<$state>::new())
    };
}

impl FSMTransition for ShmClockState<Unknown> {
    /// Implement the transitions from the Unknown FSM state.
    fn transition(&self, chrony: ChronyClockStatus) -> Box<dyn FSMState> {
        match chrony {
            ChronyClockStatus::Unknown => bstate!(Unknown),
            ChronyClockStatus::Synchronized => bstate!(Synchronized),
            ChronyClockStatus::FreeRunning => bstate!(FreeRunning),
        }
    }
}

impl FSMTransition for ShmClockState<Synchronized> {
    /// Implement the transitions from the Synchronized FSM state.
    fn transition(&self, chrony: ChronyClockStatus) -> Box<dyn FSMState> {
        match chrony {
            ChronyClockStatus::Unknown => bstate!(Unknown),
            ChronyClockStatus::Synchronized => bstate!(Synchronized),
            ChronyClockStatus::FreeRunning => bstate!(FreeRunning),
        }
    }
}

impl FSMTransition for ShmClockState<FreeRunning> {
    /// Implement the transitions from the FreeRunning FSM state.
    fn transition(&self, chrony: ChronyClockStatus) -> Box<dyn FSMState> {
        match chrony {
            ChronyClockStatus::Unknown => bstate!(Unknown),
            ChronyClockStatus::Synchronized => bstate!(Synchronized),
            ChronyClockStatus::FreeRunning => bstate!(FreeRunning),
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

    /// Assert that creating a FSM defaults to the Unknown state.
    #[test]
    fn test_entry_point_to_fsm() {
        let state = ShmClockState::default();
        assert_eq!(state.value(), ClockStatus::Unknown);
    }

    /// Assert the clock status value return by each state is correct.
    #[test]
    fn test_state_and_value() {
        let state = bstate!(Unknown);
        assert_eq!(state.value(), ClockStatus::Unknown);

        let state = bstate!(Synchronized);
        assert_eq!(state.value(), ClockStatus::Synchronized);

        let state = bstate!(FreeRunning);
        assert_eq!(state.value(), ClockStatus::FreeRunning);
    }

    #[test]
    fn test_transition_from_unknown() {
        for chrony_status in _helper_generate_chrony_status() {
            let state = bstate!(Unknown);
            let state = state.transition(chrony_status);

            if chrony_status == ChronyClockStatus::Unknown {
                assert_eq!(state.value(), ClockStatus::Unknown);
            } else if chrony_status == ChronyClockStatus::Synchronized {
                assert_eq!(state.value(), ClockStatus::Synchronized);
            } else if chrony_status == ChronyClockStatus::FreeRunning {
                assert_eq!(state.value(), ClockStatus::FreeRunning);
            }
        }
    }

    #[test]
    fn test_transition_from_freerunning() {
        for chrony_status in _helper_generate_chrony_status() {
            let state = bstate!(FreeRunning);
            let state = state.transition(chrony_status);

            if chrony_status == ChronyClockStatus::Unknown {
                assert_eq!(state.value(), ClockStatus::Unknown);
            } else if chrony_status == ChronyClockStatus::Synchronized {
                assert_eq!(state.value(), ClockStatus::Synchronized);
            } else if chrony_status == ChronyClockStatus::FreeRunning {
                assert_eq!(state.value(), ClockStatus::FreeRunning);
            }
        }
    }

    #[test]
    fn test_transition_from_synchronized() {
        for chrony_status in _helper_generate_chrony_status() {
            let state = bstate!(Synchronized);
            let state = state.transition(chrony_status);

            if chrony_status == ChronyClockStatus::Unknown {
                assert_eq!(state.value(), ClockStatus::Unknown);
            } else if chrony_status == ChronyClockStatus::Synchronized {
                assert_eq!(state.value(), ClockStatus::Synchronized);
            } else if chrony_status == ChronyClockStatus::FreeRunning {
                assert_eq!(state.value(), ClockStatus::FreeRunning);
            }
        }
    }

    /// Assert that apply_chrony is functional.
    #[test]
    fn test_apply_chrony() {
        let state = bstate!(Synchronized);
        let state = state.apply_chrony(ChronyClockStatus::Unknown);
        assert_eq!(state.value(), ClockStatus::Unknown);
    }
}
