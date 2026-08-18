//! DNS IO specific code and implementations
#![expect(dead_code)]

// IO task which holds the state of the DNS pool and performs DNS resolve actions
mod resolver;

// IO task which performs NTP requests to a DNS-resolved address
pub mod ntp_source;

/// Struct that lives in [`SourceIO`](super::SourceIO)
/// to act as the interface to a DNS pool
#[derive(Debug)]
pub struct Pool {
    pool_domain: String,
}

impl Pool {
    /// Constructor
    pub fn new(pool_domain: String) -> Self {
        Self { pool_domain }
    }
}
