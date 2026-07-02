//! An NTP event

use crate::time::{EstimateDuration, EstimateInstant};
use serde::{Deserialize, Serialize};

/// An NTP event
///
/// Represents the NTP packet round trip as defined in RFC 5905.
///
/// NOTE: This is NOT a perfect representation of a packet. Instants and durations in this struct
/// are stored via a count of nanoseconds. However, the NTP packet format stores in a "fixed point"
/// type which tends to only have non-whole microsecond step values. We are choosing this trade-off
/// to reduce complexity in generating events. The logic for converting into NTP-specific values
/// will be codified elsewhere.
///
/// NOTE: Additionally, this type also allows storing smaller and larger values than the NTP packet
/// allows. Let's see if this affects us. We can always change later.
///
/// The client's tsc timestamps, e.g., `client_tsc_pre_time` and `client_tsc_post_time`, are
/// handled separately from this struct. See [`Event`](super::Event)
///
/// Below is an example of how this is used within a single NTP packet request-response
/// ```text
///                                             server
/// ───────────────2─────3──────────────────────────────► time
///               //      \\                             
///              //        \                             
///             //         \\                            
///            //           \\                           
///           //             \\                  client  
///  ─────0───1───────────────4─────5───────────────────► time
///
/// Where:
/// 0: `client_tsc_pre_time` - Client TSC read prior to sending an NTP request (tsc timestamp stored as `Event::client_tsc_pre_time`)
/// 1: `client_system_send_time` - Client system clock read when sending an NTP request
/// 2: `server_system_recv_time` - Server receives an NTP request and stores its recv system time
/// 3: `server_system_send_time` - Server sends an NTP response with its send system time stored
/// 4: `client_system_recv_time` - Client system clock read when receiving an NTP response
/// 5: `client_tsc_post_time` - Client TSC read after receiving an NTP response (tsc timestamp stored as `Event::client_tsc_post_time`)
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ntp {
    /// The server's system time when the server receives the request
    ///
    /// This is the value the NTP server writes to the "Receive Timestamp" field of the NTP packet
    pub server_system_recv_time: EstimateInstant,
    /// The server's system time when the server sends a response
    ///
    /// This is the value the NTP server writes to the "Transmit Timestamp" field of the NTP packet
    pub server_system_send_time: EstimateInstant,
    /// The client's system timestamps within the Ntp packet exchange
    ///
    /// These are grouped separately as they are not critical in `ff-tester`.
    /// For instance, while they are valuable in data collection, they are not generated
    /// in NTP simulations
    #[serde(flatten)]
    pub client_system_times: Option<ClientSystemTimes>,
    /// The root delay, as the NTP server writes to the "Root Delay" field of the NTP packet
    pub root_delay: EstimateDuration,
    /// The root dispersion, as the NTP server writes to the "Root Dispersion" field of the NTP packet
    pub root_dispersion: EstimateDuration,
    /// Event source, e.g. the IP address of the NTP server
    ///
    /// Just has to be a unique identifier
    pub source_id: String,
}

/// The client's system timestamps within an Ntp packet exchange
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientSystemTimes {
    /// The client's system time when the client sends a request
    pub client_system_send_time: EstimateInstant,
    /// The client's system time when the client receives the response
    pub client_system_recv_time: EstimateInstant,
}
