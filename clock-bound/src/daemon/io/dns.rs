//! DNS IO specific code and implementations
#![expect(dead_code)]

// IO task which holds the state of the DNS pool and performs DNS resolve actions

mod resolver {} // todo

// IO task which performs NTP requests to
mod ntp_source {}

/// Struct that lives in [`SourceIO`](super::SourceIO) to act as the interface to a DNS pool
pub struct DnsPool {}
