//! Messages that can be handled by [`Resolver`]

use std::net::IpAddr;

/// Message enum that can be handled by a running [`Resolver`] task
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    ///  Notify resolver of a new IO task
    ///
    /// See [`RegisterAddr`] for more info
    RegisterAddr(RegisterAddr),
    /// Notify resolver that an NTP server is unreachable
    ///
    /// See [`UnreachableAddr`] for more info
    UnreachableAddr(UnreachableAddr),
    /// Notify resolver that an NTP task is shutting down
    ///
    /// See [`NtpTaskShutdown`] for more info
    NtpTaskShutdown(NtpTaskShutdown),
}

/// Notify resolver that an NTP IO task has started
///
/// This is sent by DNS Pool NTP task after spawned by the
/// [`DnsPool`](crate::daemon::io::dns::DnsPool) struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterAddr {
    pub addr: IpAddr,
}

/// Notify resolver that an NTP server is unreachable
///
/// This is sent by DNS Pool NTP task when an error occurs
/// during communication with a server. And that it should be
/// removed from the daemon
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreachableAddr {
    pub addr: IpAddr,
}

/// Notify resolver that an NTP task is shutting down
///
/// This is send by the Pool NTP task as part of the shutdown or
/// unreachable process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtpTaskShutdown {
    pub addr: IpAddr,
}
