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

/// Poll intervals of the three source types, in the order of their expected accuracy — PHC,
/// EC2 link-local NTP, other NTP servers — the order used for sources throughout the code and
/// for the fields of `FfSyncParams`. They size the estimators' buffers and bound the parameter
/// windows from below (`FfSyncParams::validate`); for now, they are not tunables.
pub(crate) const POLL_INTERVALS: [crate::daemon::time::Duration; 3] = [
    Phc::POLL_INTERVAL,
    AmazonTimeSync::POLL_INTERVAL,
    NtpSource::POLL_INTERVAL,
];

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

impl SourceInfo {
    /// Returns `true` when `self` and `other` refer to the same clock source.
    ///
    /// Identity is the variant plus the socket address for the NTP-based
    /// sources (`AmazonTimeSync`/`NtpSource`), or the device path for `Phc`.
    /// `AmazonTimeSync` and `NtpSource` are different variants and are never
    /// considered the same source, even if they happened to share an address.
    ///
    /// Stratum is deliberately excluded from this comparison.
    pub fn same_source(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::AmazonTimeSync(a, _), Self::AmazonTimeSync(b, _))
            | (Self::NtpSource(a, _), Self::NtpSource(b, _)) => a == b,
            (Self::Phc(a), Self::Phc(b)) => a == b,
            _ => false,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::event::Stratum;

    #[test]
    fn phc_with_same_device_path_is_same_source() {
        let a = SourceInfo::Phc(DevicePath::from("/dev/ptp0"));
        let b = SourceInfo::Phc(DevicePath::from("/dev/ptp0"));
        assert!(a.same_source(&b));
    }

    #[test]
    fn phc_with_different_device_path_is_not_same_source() {
        let a = SourceInfo::Phc(DevicePath::from("/dev/ptp0"));
        let b = SourceInfo::Phc(DevicePath::from("/dev/ptp1"));
        assert!(!a.same_source(&b));
    }

    #[test]
    fn amazon_time_sync_with_same_address_is_same_source() {
        let addr: SocketAddr = "169.254.169.123:123".parse().unwrap();
        let a = SourceInfo::AmazonTimeSync(addr, Stratum::ONE);
        let b = SourceInfo::AmazonTimeSync(addr, Stratum::ONE);
        assert!(a.same_source(&b));
    }

    #[test]
    fn amazon_time_sync_stratum_change_is_still_same_source() {
        // Stratum is deliberately excluded from source identity.
        let addr: SocketAddr = "169.254.169.123:123".parse().unwrap();
        let a = SourceInfo::AmazonTimeSync(addr, Stratum::ONE);
        let b = SourceInfo::AmazonTimeSync(addr, Stratum::TWO);
        assert!(a.same_source(&b));
    }

    #[test]
    fn amazon_time_sync_with_different_address_is_not_same_source() {
        let a = SourceInfo::AmazonTimeSync("169.254.169.123:123".parse().unwrap(), Stratum::ONE);
        let b = SourceInfo::AmazonTimeSync("169.254.169.101:123".parse().unwrap(), Stratum::ONE);
        assert!(!a.same_source(&b));
    }

    #[test]
    fn amazon_time_sync_and_ntp_source_with_same_address_are_not_same_source() {
        // Different variants are never the same source, even with a shared address.
        let addr: SocketAddr = "169.254.169.123:123".parse().unwrap();
        let a = SourceInfo::AmazonTimeSync(addr, Stratum::ONE);
        let b = SourceInfo::NtpSource(addr, Stratum::ONE);
        assert!(!a.same_source(&b));
    }

    #[test]
    fn phc_and_ntp_source_are_never_same_source() {
        let a = SourceInfo::Phc(DevicePath::from("/dev/ptp0"));
        let b = SourceInfo::NtpSource("169.254.169.123:123".parse().unwrap(), Stratum::ONE);
        assert!(!a.same_source(&b));
    }
}
