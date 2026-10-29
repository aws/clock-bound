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

/// Identifies a clock source.
///
/// Used for logging and for tracking which source is selected. Where the stratum of the source's
/// last response is needed, see [`SourceInfoWithStratum`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceInfo {
    AmazonTimeSync(SocketAddr),
    NtpSource(SocketAddr),
    Phc(DevicePath),
}

impl SourceInfo {
    /// The source's identifier for logs: its address (`ip:port`) or device path.
    pub fn identifier(&self) -> String {
        match self {
            Self::AmazonTimeSync(addr) | Self::NtpSource(addr) => addr.to_string(),
            Self::Phc(device_path) => device_path.to_string(),
        }
    }
}

/// A [`SourceInfo`] plus the stratum reported by the source's latest NTP response.
///
/// Only the NTP-based variants carry a stratum. Convert to a [`SourceInfo`] (via [`From`]) to
/// drop it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceInfoWithStratum {
    AmazonTimeSync(SocketAddr, Stratum),
    NtpSource(SocketAddr, Stratum),
    Phc(DevicePath),
}

impl From<&SourceInfoWithStratum> for SourceInfo {
    fn from(value: &SourceInfoWithStratum) -> Self {
        match value {
            SourceInfoWithStratum::AmazonTimeSync(addr, _) => Self::AmazonTimeSync(*addr),
            SourceInfoWithStratum::NtpSource(addr, _) => Self::NtpSource(*addr),
            SourceInfoWithStratum::Phc(device_path) => Self::Phc(device_path.clone()),
        }
    }
}

impl From<SourceInfoWithStratum> for SourceInfo {
    fn from(value: SourceInfoWithStratum) -> Self {
        match value {
            SourceInfoWithStratum::AmazonTimeSync(addr, _) => Self::AmazonTimeSync(addr),
            SourceInfoWithStratum::NtpSource(addr, _) => Self::NtpSource(addr),
            SourceInfoWithStratum::Phc(device_path) => Self::Phc(device_path),
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

    #[test]
    fn phc_with_same_device_path_is_same_source() {
        let a = SourceInfo::Phc(DevicePath::from("/dev/ptp0"));
        let b = SourceInfo::Phc(DevicePath::from("/dev/ptp0"));
        assert_eq!(a, b);
    }

    #[test]
    fn phc_with_different_device_path_is_not_same_source() {
        let a = SourceInfo::Phc(DevicePath::from("/dev/ptp0"));
        let b = SourceInfo::Phc(DevicePath::from("/dev/ptp1"));
        assert_ne!(a, b);
    }

    #[test]
    fn amazon_time_sync_with_same_address_is_same_source() {
        let addr: SocketAddr = "169.254.169.123:123".parse().unwrap();
        assert_eq!(
            SourceInfo::AmazonTimeSync(addr),
            SourceInfo::AmazonTimeSync(addr)
        );
    }

    #[test]
    fn amazon_time_sync_with_different_address_is_not_same_source() {
        let a = SourceInfo::AmazonTimeSync("169.254.169.123:123".parse().unwrap());
        let b = SourceInfo::AmazonTimeSync("169.254.169.101:123".parse().unwrap());
        assert_ne!(a, b);
    }

    #[test]
    fn amazon_time_sync_and_ntp_source_with_same_address_are_not_same_source() {
        // Different variants are never the same source, even with a shared address.
        let addr: SocketAddr = "169.254.169.123:123".parse().unwrap();
        assert_ne!(
            SourceInfo::AmazonTimeSync(addr),
            SourceInfo::NtpSource(addr)
        );
    }

    #[test]
    fn phc_and_ntp_source_are_never_same_source() {
        let a = SourceInfo::Phc(DevicePath::from("/dev/ptp0"));
        let b = SourceInfo::NtpSource("169.254.169.123:123".parse().unwrap());
        assert_ne!(a, b);
    }

    #[test]
    fn stratum_is_dropped_when_converting_to_source_info() {
        // Sources differing only by stratum convert to the same `SourceInfo`.
        let addr: SocketAddr = "169.254.169.123:123".parse().unwrap();
        let one = SourceInfoWithStratum::AmazonTimeSync(addr, Stratum::ONE);
        let two = SourceInfoWithStratum::AmazonTimeSync(addr, Stratum::TWO);
        assert_ne!(one, two);
        assert_eq!(SourceInfo::from(&one), SourceInfo::from(&two));
        assert_eq!(SourceInfo::from(one), SourceInfo::AmazonTimeSync(addr));
    }

    #[test]
    fn phc_converts_to_source_info() {
        let with = SourceInfoWithStratum::Phc(DevicePath::from("/dev/ptp0"));
        assert_eq!(
            SourceInfo::from(&with),
            SourceInfo::Phc(DevicePath::from("/dev/ptp0"))
        );
    }
}
