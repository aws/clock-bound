//! Messages handled by the Daemon task

use std::net::SocketAddr;

/// Top level message by the daemon task from DNS resolvers
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dns {
    /// Add a new NTP source to a pool
    ///
    /// For more info see [`AddPoolAddr`]
    AddPoolAddr(AddPoolAddr),
    /// Remove an existing NTP source from a pool
    ///
    /// For more info see [`RemovePoolAddr`]
    RemovePoolAddr(RemovePoolAddr),
}

/// Initialize a new NTP address in a pool
///
/// This message is sent by the [`Resolver`](crate::daemon::io::dns)
/// when a new host is resolved and should be constructed in the rest
/// of the app
// TODO: fixup the doc link when the `Resolver` struct is merged in
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddPoolAddr {
    /// The domain of the pool to add an address to
    pub pool_domain: String,
    /// The address to add to the pool
    pub addr: SocketAddr,
}

/// Remove an existing NTP address from a pool
///
/// This message is sent by the [`Resolver`](crate::daemon::io::dns)
/// when a host should be removed. Could happen if
/// the host stops responding
// TODO: fixup the doc link when the `Resolver` struct is merged in
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovePoolAddr {
    /// The domain of the pool to remove an address from
    pub pool_domain: String,
    /// The address to remove from the pool
    pub addr: SocketAddr,
}
