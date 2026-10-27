//! ClockBound

/// Backwards compatible paths and names for the ClockBound 2.X client API.
///
/// New code should use the crate-root items instead, e.g. [`clock_bound::Client`](crate::Client) rather than
/// `clock_bound::client::ClockBoundClient`.
pub mod client {
    pub use crate::Client as ClockBoundClient;
    pub use crate::ClockStatus;
    pub use crate::Error as ClockBoundError;
    pub use crate::ErrorKind as ClockBoundErrorKind;
    pub use crate::Now as ClockBoundNowResult;
    pub use crate::SHM_DEFAULT_PATH as CLOCKBOUND_SHM_DEFAULT_PATH;
    pub use crate::VMCLOCK_DEFAULT_PATH as VMCLOCK_SHM_DEFAULT_PATH;
}

// name to make it easier to keep typedefs of ClockBound 2.X types
mod client_impl;

pub use client_impl::Client;
pub use client_impl::ClockStatus;
pub use client_impl::Error;
pub use client_impl::ErrorKind;
pub use client_impl::Now;

mod shm;

mod vmclock;

pub use crate::shm::SHM_DEFAULT_PATH;
pub use crate::vmclock::shm::VMCLOCK_DEFAULT_PATH;

// Exposed only for the vmclock-write test tooling (test/vmclock-updater and,
// temporarily, test/amazon-time-sync). Not part of the stable public API.
#[cfg(feature = "test-utils")]
pub mod test_utils {
    pub use crate::{
        shm::{ClockErrorBound, ClockErrorBoundGeneric, ClockErrorBoundLayoutVersion},
        vmclock::{
            shm::{VMClockClockStatus, VMClockShmBody},
            shm_writer::{VMClockShmWrite, VMClockShmWriter},
        },
    };
}

#[cfg(any(feature = "daemon", feature = "algorithm"))]
pub mod daemon;
