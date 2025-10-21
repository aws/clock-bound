//! Events used by `ff-tester`. For creating simulations and for capturing real events.
//!
//! These structs are serializable and versioned to allow for easy breaking changes while continuing to support older captures when desired.
//!
//! The focus for this module is on serialization only. While it may hold a large number of types that may look tempting to implement
//! mathematical functions on, it's best to put that logic into a separate crate that is focused only on that. Rationale is that
//! adding fields to `serde` data types constitutes a potentially breaking change, whereas we would want to not be limited when working
//! on clock transformations.

use serde::{Deserialize, Serialize};

pub mod v1;

/// The type for serializing or deserializing scenarios throughout the lifecycle of `ff-tester`
///
/// Versioned enum to enable breaking changes and backwards compatibility
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Scenario {
    /// See [`v1`] for more information about what this entails.
    V1(v1::Scenario),
}

/// Serialize using json to a writer
///
/// # Errors
///
/// Errors if unable to write
pub fn serialize_to_writer_pretty<W: std::io::Write>(
    scenario: &Scenario,
    writer: W,
) -> Result<(), serde_json::Error> {
    serde_json::to_writer_pretty(writer, scenario)
}

/// Deserialize using json from a reader
///
/// # Errors
/// Returns an error if failed to deserialize
pub fn deserialize_from_reader<R: std::io::Read>(reader: R) -> Result<Scenario, serde_json::Error> {
    serde_json::from_reader(reader)
}

/// Deserialize from a json string
///
/// # Errors
/// Returns an error if failed to deserialize
pub fn deserialize_from_str(json: &str) -> Result<Scenario, serde_json::Error> {
    serde_json::from_str(json)
}
