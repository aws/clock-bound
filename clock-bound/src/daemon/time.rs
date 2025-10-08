//! Simple time library for the `ClockBound` time synchronization daemon
//!
//! Other time libraries do not meet our needs, as we make heavy usage of time stamp counters (TSCs)
//! for the bulk of our processing. These values are more low-level than those seen in `chrono` or other time types

pub mod inner;
pub mod instant;
pub mod timex;
pub mod tsc;

pub use instant::{Duration, Instant};
pub use tsc::{TscCount, TscDiff};
