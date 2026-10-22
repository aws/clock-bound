//! ClockBound

pub mod client;

mod shm;

mod vmclock;

pub use crate::shm::{ClockBoundNowResult, ClockStatus, SHM_DEFAULT_PATH};
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
