//! Flavours of clock sync algorithms
//!
//! This is where math happens

pub mod event_buffer;

mod ntp;
pub use ntp::Ntp;
