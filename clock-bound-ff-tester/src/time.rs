//! Simple time library for usage in `ff-tester` and a lost opportunity to have a library called "eff time"
//!
//! Other time libraries do not meet our needs, as `ClockBound` uses hardware counters
//! for the bulk of it's processing. These values are lower-level and abstractier than those seen in `chrono` and `std::time`.
//!
//! Furthermore, `ff-tester` during simulations must make considerations based on a mythical "true time", where the precise, accurate, and otherwise
//! exact time is known a-priori. Furthermore, it must also make distinctions between "time estimates", where a best effort of time is known.

// These types are unchanged from clock-bound
pub use clock_bound::daemon::time::inner;
pub use clock_bound::daemon::time::tsc::{Frequency, Period, Skew, TscCount, TscDiff};

mod true_instant;
pub use true_instant::{AssumeTrue, DemoteToEstimate, TrueDuration, TrueInstant};

mod estimate_instant;
pub use estimate_instant::{CbBridge, EstimateDuration, EstimateInstant};

mod series;
pub use series::Series;
