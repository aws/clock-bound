//! Logging infrastructure for ClockBound.
//!
//! This module provides:
//! - Target constants for routing tracing events to specific log files
//! - A custom [`StructuredLogLayer`] for writing structured JSON log files
//! - The tracing [`subscriber`] setup that wires everything together

pub mod layer;
pub mod subscriber;

/// Tracing target for the primer reproducibility log.
pub const PRIMER_TARGET: &str = "clock_bound::primer";

/// Tracing target for the clock metrics log.
pub const CLOCK_METRICS_TARGET: &str = "clock_bound::clock_metrics";
