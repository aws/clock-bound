//! VMClock access

pub mod shm;
pub mod shm_reader;

#[cfg(any(test, feature = "test-utils"))]
pub mod shm_writer;
