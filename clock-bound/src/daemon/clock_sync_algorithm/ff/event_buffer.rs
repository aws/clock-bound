//! Local and Estimate ring buffers used within a Feed Forward Clock Sync Algorithm

mod local;
pub use local::Local;

mod estimate;
pub use estimate::Estimate;
