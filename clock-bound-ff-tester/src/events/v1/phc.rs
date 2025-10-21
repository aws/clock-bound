//! A PTP Hardware Clock (PHC) event

use crate::time::{EstimateDuration, EstimateInstant};
use serde::{Deserialize, Serialize};

/// A PHC Event
///
/// Represents the PHC clock synchronization event structure.
/// Where the PHC clock synchronization algorithm reads the TSC, PHC,
/// and TSC in that order before correcting the system clock.
///
/// The client's tsc timestamps, e.g., `client_tsc_pre_time` and `client_tsc_post_time`, are
/// handled separately from this struct. See [`Event`](super::Event)
///
/// Below is an example of how this is used within a single PHC read sequence
/// ```text
///                                         PHC
/// ────────────────3─────────────────────────────► time
///               // \\                             
///              //   \                             
///             //    \\                            
///            //      \\                           
///           //        \\                  client  
///  ─────0───1──────────4────5───────────────────► time
///
/// Where:
/// 0: `client_tsc_pre_time` - Client TSC read prior to reading the PHC (tsc timestamp stored as `Event::client_tsc_pre_time`)
/// 1: `client_system_pre_time` - Client system clock read before reading the PHC
/// 2: `phc_time` - PHC timestamp when read by the client
/// 3: `client_system_post_time` - Client system clock read after reading the PHC
/// 4: `client_tsc_post_time` - Client TSC read post reading the PHC (tsc timestamp stored as `Event::client_tsc_post_time`)
/// ```
///
/// Note, that the generic PHC device has no inherent concept of clock error bound, however the EC2 PHC device exposes the
/// clock error bound through a sysfs device file. If this is supported, then PHC value can have the associated clock error bound
/// as well
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Phc {
    /// The PHC timestamp when read by the client
    pub phc_time: EstimateInstant,
    #[serde(flatten)]
    /// The client's system timestamps surrounding PHC reads
    ///
    /// These are grouped separately as they are not critical in `ff-tester`.
    pub client_system_times: Option<ClientSystemTimes>,
    /// Clock error measurement of the reading (from a separate sysfs file, if it exists)
    pub clock_error_bound: Option<EstimateDuration>,
    /// Event source identifier, e.g. the "ptp0"
    pub source_id: String,
}

/// The client's system timestamps surrounding PHC reads
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientSystemTimes {
    /// The client's system time before the client reads the PHC
    pub client_system_pre_time: EstimateInstant,
    /// The client's system time after the client reads the PHC
    pub client_system_post_time: EstimateInstant,
}
