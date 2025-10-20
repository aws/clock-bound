//! Reference clock sources
//!
//! Used to separate instantiations of [`ff`](super::ff).
//! Just because a link local connection and a NTP source use the same underlying
//! [`ff::Ntp`](super::ff::Ntp) algorithm, does not mean everything about the sources are the same.
//!
//! This module contains wrapping logic around [`ff`](super::ff) to enable stronger separation of
//! concerns between different source types.

mod link_local;

pub use link_local::LinkLocal;
