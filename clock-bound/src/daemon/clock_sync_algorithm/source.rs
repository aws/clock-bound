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
