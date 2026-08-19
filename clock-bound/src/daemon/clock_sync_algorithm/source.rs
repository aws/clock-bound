//! Reference clock sources
//!
//! Used to separate instantiations of [`ff`](super::ff).
//! Just because the Amazon Time Sync and an NTP source use the same underlying
//! [`ff::Ntp`](super::ff::Ntp) algorithm, does not mean everything about the sources are the same.
//!
//! This module contains wrapping logic around [`ff`](super::ff) to enable stronger separation of
//! concerns between different source types.

mod amazon_time_sync;
mod ntp_source;
mod phc;

pub use amazon_time_sync::AmazonTimeSync;
pub use ntp_source::NtpSource;
pub use phc::Phc;

use std::fmt::{self, Display, Formatter};
use std::net::SocketAddr;
use std::sync::Arc;

use crate::daemon::event::Stratum;

/// Identifies which clock source produced a [`SyncParameters`](super::SyncParameters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceInfo {
    AmazonTimeSync(SocketAddr, Stratum),
    NtpSource(SocketAddr, Stratum),
    Phc(DevicePath),
}

/// Filesystem path identifying a device-backed clock source, e.g. `/dev/ptp0`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DevicePath(Arc<str>);

impl DevicePath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for DevicePath {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for DevicePath {
    fn from(value: String) -> Self {
        Self(Arc::from(value))
    }
}

impl From<&str> for DevicePath {
    fn from(value: &str) -> Self {
        Self(Arc::from(value))
    }
}
