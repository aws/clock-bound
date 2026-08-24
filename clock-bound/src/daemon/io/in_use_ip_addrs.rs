//! Tracks IP addresses currently in use across DNS pool resolvers.
//!
//! [`InUseIpAddrs`] provides thread-safe, shared tracking of which IP addresses are actively
//! being used by NTP IO tasks. This prevents multiple resolvers from launching tasks against the
//! same IP address.

use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use thiserror::Error;

/// Thread-safe tracker of IP addresses currently in use by NTP IO tasks.
///
/// Designed to be cloned into each [`Resolver`](super::dns::resolver::Resolver) so that multiple
/// resolvers sharing the same IP pool avoid launching duplicate tasks against the same address.
//
// Uses a vec for storage as opposed to hashmap, as performance is faster when small (less than 100 elements)
#[derive(Debug, Clone)]
pub struct InUseIpAddrs {
    addrs: Arc<Mutex<Vec<IpAddr>>>,
}

impl InUseIpAddrs {
    /// Construct a new, empty `InUseIpAddrs`.
    pub fn new() -> Self {
        Self {
            addrs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Add an IP address to the in-use set.
    ///
    /// Returns `Ok(())` if the address was successfully added.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError`] if the address was already tracked.
    pub fn add(&self, addr: IpAddr) -> Result<(), KeyError> {
        #[expect(clippy::missing_panics_doc, reason = "not handling mutex poison")]
        let mut guard = self.addrs.lock().unwrap();
        if guard.contains(&addr) {
            return Err(KeyError { addr });
        }
        guard.push(addr);
        Ok(())
    }

    /// Remove an IP address from the in-use set
    ///
    /// Returns `Ok(())` if the address was found and removed.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError`] if the address was not present.
    pub fn remove(&self, addr: &IpAddr) -> Result<(), KeyError> {
        #[expect(clippy::missing_panics_doc, reason = "not handling mutex poison")]
        let mut guard = self.addrs.lock().unwrap();
        if let Some(pos) = guard.iter().position(|a| a == addr) {
            guard.swap_remove(pos);
            Ok(())
        } else {
            Err(KeyError { addr: *addr })
        }
    }

    /// Atomically inspect the current in-use addresses and add new ones.
    ///
    /// The closure receives the current slice of in-use addresses and returns a
    /// `Vec<IpAddr>` of new addresses to add. If the closure returns an empty vec,
    /// the lock is released without modification.
    ///
    /// Returns the vec returned by the closure.
    pub fn transact_add<F>(&self, f: F) -> Vec<IpAddr>
    where
        F: FnOnce(&[IpAddr]) -> Vec<IpAddr>,
    {
        #[expect(clippy::missing_panics_doc, reason = "not handling mutex poison")]
        let mut guard = self.addrs.lock().unwrap();
        tracing::debug!("global guard contents (before): {:?}", guard);
        let new_addrs = f(&guard);
        for addr in &new_addrs {
            if !guard.contains(addr) {
                guard.push(*addr);
            }
        }
        tracing::debug!("global guard contents (after): {:?}", guard);
        new_addrs
    }

    /// Returns a snapshot of the currently tracked addresses.
    #[cfg(test)]
    pub fn snapshot(&self) -> Vec<IpAddr> {
        self.addrs.lock().unwrap().clone()
    }
}

impl Default for InUseIpAddrs {
    fn default() -> Self {
        Self::new()
    }
}

/// Error returned when a key is invalid for an operation
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("Invalid IpAddress {addr}")]
pub struct KeyError {
    /// The offending key
    pub addr: IpAddr,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn add_returns_ok_for_new_addr() {
        let tracker = InUseIpAddrs::new();
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        assert!(tracker.add(addr).is_ok());
    }

    #[test]
    fn add_returns_key_error_for_duplicate_addr() {
        let tracker = InUseIpAddrs::new();
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        tracker.add(addr).unwrap();
        let err = tracker.add(addr).unwrap_err();
        assert_eq!(err, KeyError { addr });
    }

    #[test]
    fn remove_returns_ok_when_present() {
        let tracker = InUseIpAddrs::new();
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        tracker.add(addr).unwrap();
        assert!(tracker.remove(&addr).is_ok());
    }

    #[test]
    fn remove_returns_key_error_when_absent() {
        let tracker = InUseIpAddrs::new();
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let err = tracker.remove(&addr).unwrap_err();
        assert_eq!(err, KeyError { addr });
    }

    #[test]
    fn remove_uses_swap_remove_semantics() {
        let tracker = InUseIpAddrs::new();
        let addr1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let addr2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        let addr3 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3));

        tracker.add(addr1).unwrap();
        tracker.add(addr2).unwrap();
        tracker.add(addr3).unwrap();

        // Remove from middle — swap_remove moves last element into removed position
        tracker.remove(&addr1).unwrap();

        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.contains(&addr2));
        assert!(snapshot.contains(&addr3));
    }

    #[test]
    fn transact_add_inserts_returned_addrs() {
        let tracker = InUseIpAddrs::new();
        let addr1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let addr2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        let result = tracker.transact_add(|_current| vec![addr1, addr2]);

        assert_eq!(result, vec![addr1, addr2]);
        let snapshot = tracker.snapshot();
        assert!(snapshot.contains(&addr1));
        assert!(snapshot.contains(&addr2));
    }

    #[test]
    fn transact_add_does_nothing_on_empty_vec() {
        let tracker = InUseIpAddrs::new();

        let result = tracker.transact_add(|_current| vec![]);

        assert!(result.is_empty());
        assert!(tracker.snapshot().is_empty());
    }

    #[test]
    fn transact_add_receives_current_addrs() {
        let tracker = InUseIpAddrs::new();
        let existing = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        tracker.add(existing).unwrap();

        let new_addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        let result = tracker.transact_add(|current| {
            assert_eq!(current, &[existing]);
            vec![new_addr]
        });

        assert_eq!(result, vec![new_addr]);
        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.contains(&existing));
        assert!(snapshot.contains(&new_addr));
    }

    #[test]
    fn transact_add_does_not_duplicate_existing() {
        let tracker = InUseIpAddrs::new();
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        tracker.add(addr).unwrap();

        tracker.transact_add(|_current| vec![addr]);

        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.len(), 1);
    }

    #[test]
    fn clone_shares_state() {
        let tracker = InUseIpAddrs::new();
        let clone = tracker.clone();
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        tracker.add(addr).unwrap();
        assert!(clone.snapshot().contains(&addr));
    }

    #[test]
    fn default_creates_empty_tracker() {
        let tracker = InUseIpAddrs::default();
        assert!(tracker.snapshot().is_empty());
    }
}
