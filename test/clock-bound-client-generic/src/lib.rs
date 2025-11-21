//! A testing library that exposes a "generic" clock bound client enum that wraps both ClockBound v2 and v3 clients.
//!
//! This library intentionally imports the `clock_bound_client` v2.0 crate to fully simulate/validate backward compatibility
//! between v2.0 clients and v3.0 daemon.

use clap::ValueEnum;
use clock_bound::daemon::time::Duration;
use clock_bound::daemon::time::Instant;
use nix::sys::time::TimeSpec;

use clock_bound::client as clock_bound_client_v3;
use clock_bound::shm as clockbound_shm_v3;
use clock_bound_client as clock_bound_client_v2;

const NSECS_PER_SEC: i64 = 1_000_000_000;

use thiserror::Error;

#[derive(Error, Clone, Debug, serde::Serialize)]
pub enum ClockBoundClientError {
    #[error("clockbound client v2 read failed.")]
    V2Error,
    #[error("clockbound client v3 read failed.")]
    V3Error,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, serde::Serialize)]
pub enum ClockStatus {
    Unknown = 0,
    Synchronized = 1,
    FreeRunning = 2,
    Disrupted = 3,
}
impl From<clock_bound_client_v2::ClockStatus> for ClockStatus {
    fn from(value: clock_bound_client_v2::ClockStatus) -> Self {
        match value {
            clock_bound_client_v2::ClockStatus::Unknown => Self::Unknown,
            clock_bound_client_v2::ClockStatus::Synchronized => Self::Synchronized,
            clock_bound_client_v2::ClockStatus::FreeRunning => Self::FreeRunning,
            clock_bound_client_v2::ClockStatus::Disrupted => Self::Disrupted,
        }
    }
}
impl From<clock_bound_client_v3::ClockStatus> for ClockStatus {
    fn from(value: clock_bound_client_v3::ClockStatus) -> Self {
        match value {
            clock_bound_client_v3::ClockStatus::Unknown => Self::Unknown,
            clock_bound_client_v3::ClockStatus::Synchronized => Self::Synchronized,
            clock_bound_client_v3::ClockStatus::FreeRunning => Self::FreeRunning,
            clock_bound_client_v3::ClockStatus::Disrupted => Self::Disrupted,
        }
    }
}

fn timespec_to_nsecs(timespec: TimeSpec) -> i64 {
    timespec.tv_sec() * NSECS_PER_SEC + timespec.tv_nsec()
}

#[derive(PartialEq, Clone, Debug)]
pub struct ClockBoundNowResult {
    pub timestamp: i64,
    pub ceb: i64,
    pub clock_status: ClockStatus,
}
impl From<clock_bound_client_v2::ClockBoundNowResult> for ClockBoundNowResult {
    fn from(value: clock_bound_client_v2::ClockBoundNowResult) -> Self {
        let midpoint = (value.latest + value.earliest) / 2;
        let bound = (value.latest - value.earliest) / 2;

        ClockBoundNowResult {
            timestamp: timespec_to_nsecs(midpoint),
            ceb: timespec_to_nsecs(bound),
            clock_status: ClockStatus::from(value.clock_status),
        }
    }
}
impl From<clockbound_shm_v3::ClockBoundNowResult> for ClockBoundNowResult {
    fn from(value: clockbound_shm_v3::ClockBoundNowResult) -> Self {
        let midpoint = (value.latest + value.earliest) / 2;
        let bound = (value.latest - value.earliest) / 2;

        ClockBoundNowResult {
            timestamp: timespec_to_nsecs(midpoint),
            ceb: timespec_to_nsecs(bound),
            clock_status: ClockStatus::from(value.clock_status),
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum ClockBoundClientVersion {
    V2,
    V3,
}

pub enum ClockBoundClient {
    ClientV2(Box<clock_bound_client_v2::ClockBoundClient>),
    ClientV3(Box<clock_bound_client_v3::ClockBoundClient>),
}
impl ClockBoundClient {
    /// Construct a new generic ClockBound client.
    ///
    /// # Panics
    /// Panics if initialization of ClockBound client fails.
    pub fn new(version: ClockBoundClientVersion) -> Self {
        match version {
            ClockBoundClientVersion::V2 => Self::ClientV2(Box::new(
                clock_bound_client_v2::ClockBoundClient::new()
                    .expect("Failed to initialize ClockBound v2 client."),
            )),
            ClockBoundClientVersion::V3 => Self::ClientV3(Box::new(
                clock_bound_client_v3::ClockBoundClient::new()
                    .expect("Failed to initialize ClockBound v3 client."),
            )),
        }
    }
    /// Retrieves the current time.
    ///
    /// # Errors
    /// Returns an error if the clock bound time retrieval fails.
    fn now(&mut self) -> Result<ClockBoundNowResult, ClockBoundClientError> {
        match self {
            Self::ClientV2(client) => Ok(ClockBoundNowResult::from(
                client.now().map_err(|_| ClockBoundClientError::V2Error)?,
            )),
            Self::ClientV3(client) => Ok(ClockBoundNowResult::from(
                client.now().map_err(|_| ClockBoundClientError::V3Error)?,
            )),
        }
    }
}

/// Convenience struct holding timestamp, error bound, and status.
/// Encapsulates the output of interest from a clock bound client read.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TimeAndBoundAndStatus {
    pub time: Instant,
    pub ceb: Duration,
    pub status: ClockStatus,
}
impl From<ClockBoundNowResult> for TimeAndBoundAndStatus {
    fn from(value: ClockBoundNowResult) -> Self {
        let time = Instant::from_nanos(value.timestamp.into());
        let ceb = Duration::from_nanos(value.ceb.into());
        let status = value.clock_status;

        TimeAndBoundAndStatus { time, ceb, status }
    }
}

pub trait ClockBoundClientClock {
    /// Retrieves timestamp, error bound, and status from a clock bound client clock.
    ///
    /// # Errors
    /// Returns an error if clock read fails.
    fn get_time_and_bound_and_status(
        &mut self,
    ) -> Result<TimeAndBoundAndStatus, ClockBoundClientError>;
}

impl ClockBoundClientClock for ClockBoundClient {
    fn get_time_and_bound_and_status(
        &mut self,
    ) -> Result<TimeAndBoundAndStatus, ClockBoundClientError> {
        Ok(TimeAndBoundAndStatus::from(self.now()?))
    }
}
