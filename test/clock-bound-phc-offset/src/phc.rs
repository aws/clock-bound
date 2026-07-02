#![allow(clippy::cast_possible_wrap)]
use clock_bound::daemon::io::tsc::{read_timestamp_counter_begin, read_timestamp_counter_end};
pub use clock_bound_client_generic::{ClockBoundClientClock, TimeAndBoundAndStatus};
use clock_bound_client_generic::{ClockBoundClientError, ClockStatus};
use nix::errno::Errno;
use std::{fs::File, path::PathBuf};

use clock_bound::daemon::time::instant::Utc;
use clock_bound::daemon::time::{ClockExt, Duration, Instant, TscCount, TscDiff};

mod ptp;
use ptp::PtpReader;

mod ceb;
use ceb::PhcClockErrorBoundReader;

mod autoconfiguration;
pub use autoconfiguration::autoconfigure_phc_reader;

use thiserror::Error;

use crate::phc::ceb::CebReadError;

#[allow(dead_code)]
#[derive(Error, Debug, Clone, serde::Serialize)]
pub enum ClockReadError {
    #[error("clock bound client read failed: `{0}`")]
    ClockBoundClient(ClockBoundClientError),
    #[error("PTP device read failed: `{0}`")]
    PtpReadFailure(String),
    #[error("PHC CEB read failed: `{0}`")]
    PhcCebReadFailure(CebReadError),
}

/// Convenience struct holding metadata pertaining to clock->ref-clock->clock read exchange.
#[allow(dead_code)]
#[derive(Debug, serde::Serialize)]
struct ExchangeMetadata {
    /// Offset between midpoint of pre/post clock read and PHC timestamp.
    /// A positive value here indicates that the clock is ahead of the PHC.
    offset: Duration,
    /// RTT representing duration between pre/post clock reads (and bounding PHC read).
    /// Period is inherited from the clock we're reading from.
    rtt_clock: Duration,
    /// RTT in representing duration between pre/post TSC reads (and bounding PHC read).
    /// Value represents elapsed TSC ticks.
    rtt_tsc: TscDiff,
}

/// Convenience struct wrapping an instant. Represents the read of a basic clock (one with no status/error-bound).
/// Really this just exists so that we can normalize the `BasicClockRefComparison` and `ClockBoundClientRefComparison` json format.
#[allow(dead_code)]
#[derive(Debug, serde::Serialize)]
struct BasicClockTime {
    time: Instant,
}
impl From<Instant> for BasicClockTime {
    fn from(value: Instant) -> Self {
        Self { time: value }
    }
}

/// Convenience struct representing all data of interest from a comparison read between a "basic clock" and the PHC.
#[allow(dead_code)]
#[derive(Debug, serde::Serialize)]
pub struct BasicClockRefComparison {
    clock_pre: BasicClockTime,
    tsc_pre: TscCount,
    ref_clock: Result<TimeAndBoundAndStatus, ClockReadError>,
    exchange_metadata: Option<ExchangeMetadata>,
    tsc_post: TscCount,
    clock_post: BasicClockTime,
}

/// Convenience struct representing all data of interest from a comparison read between a clock bound client clock and the PHC.
#[allow(dead_code)]
#[derive(Debug, serde::Serialize)]
pub struct ClockBoundClientRefComparison {
    clock_pre: Result<TimeAndBoundAndStatus, ClockReadError>,
    tsc_pre: TscCount,
    ref_clock: Result<TimeAndBoundAndStatus, ClockReadError>,
    exchange_metadata: Option<ExchangeMetadata>,
    tsc_post: TscCount,
    clock_post: Result<TimeAndBoundAndStatus, ClockReadError>,
}

#[derive(Debug)]
pub struct PhcReader {
    // we need to hold the actual File to keep the raw fd valid.
    _phc_device_file: File,
    ptp_reader: PtpReader,
    ceb_reader: PhcClockErrorBoundReader,
}
impl PhcReader {
    /// Constructs a `PhcReader` struct using the specified PHC device file and PHC CEB file path.
    /// Should not be called directly - used by autoconfiguration function.
    pub fn new(phc_device_file: File, phc_clock_error_bound_path: PathBuf) -> Self {
        let ptp_reader = PtpReader::new(&phc_device_file);
        let ceb_reader = PhcClockErrorBoundReader::new(phc_clock_error_bound_path);
        Self {
            _phc_device_file: phc_device_file,
            ptp_reader,
            ceb_reader,
        }
    }

    /// Retrieves a timestamp from the PHC device.
    ///
    /// # Errors
    /// Returns an error if the PTP system call fails.
    pub fn get_time(&self) -> Result<Instant, Errno> {
        self.ptp_reader.ptp_get_time()
    }

    fn build_response_from_phc_read(
        phc_time: Result<Instant, Errno>,
        phc_ceb: Result<Duration, CebReadError>,
    ) -> Result<TimeAndBoundAndStatus, ClockReadError> {
        let phc_time = phc_time.map_err(|e| ClockReadError::PtpReadFailure(e.to_string()))?;
        let phc_ceb = phc_ceb.map_err(ClockReadError::PhcCebReadFailure)?;
        Ok(TimeAndBoundAndStatus {
            time: phc_time,
            ceb: phc_ceb,
            status: ClockStatus::Synchronized,
        })
    }

    fn build_response_from_clock_bound_client_read(
        clock_read: Result<TimeAndBoundAndStatus, ClockBoundClientError>,
    ) -> Result<TimeAndBoundAndStatus, ClockReadError> {
        let clock_read = clock_read.map_err(|e| ClockReadError::PtpReadFailure(e.to_string()))?;
        Ok(clock_read)
    }

    /// Return details from a "basic" UTC clock to PHC comparision.
    ///
    /// # Errors
    /// Returns an error if the PHC read or CEB read operation fails.
    #[allow(clippy::missing_panics_doc)]
    pub fn get_offset_from_utc_clock<T: ClockExt<Utc>>(
        &self,
        clock: &T,
    ) -> BasicClockRefComparison {
        let clock_pre = clock.get_time();
        let tsc_pre = read_timestamp_counter_begin();
        let phc_time = self.get_time();
        let tsc_post = read_timestamp_counter_end();
        let clock_post = clock.get_time();
        let phc_ceb = self.ceb_reader.read();

        let tsc_pre = TscCount::new(tsc_pre as i64);
        let tsc_post = TscCount::new(tsc_post as i64);

        let ref_clock = Self::build_response_from_phc_read(phc_time, phc_ceb);
        let exchange_metadata = if ref_clock.is_err() {
            None
        } else {
            let mid = clock_pre.midpoint(clock_post);
            let offset = mid - ref_clock.clone().unwrap().time;
            let rtt_clock = clock_post - clock_pre;
            let rtt_tsc = tsc_post - tsc_pre;

            Some(ExchangeMetadata {
                offset,
                rtt_clock,
                rtt_tsc,
            })
        };

        BasicClockRefComparison {
            clock_pre: BasicClockTime::from(clock_pre),
            tsc_pre,
            ref_clock,
            exchange_metadata,
            tsc_post,
            clock_post: BasicClockTime::from(clock_post),
        }
    }

    /// Return comparison data from a ClockBound client clock to PHC.
    ///
    /// # Errors
    /// Returns an error if the PTP syscall, PHC read, or client read operations fail.
    #[allow(clippy::missing_panics_doc)]
    pub fn compare_to_clock_bound_client_clock<T: ClockBoundClientClock>(
        &self,
        clock: &mut T,
    ) -> ClockBoundClientRefComparison {
        let clock_pre = clock.get_time_and_bound_and_status();
        let tsc_pre = read_timestamp_counter_begin();
        let phc_time = self.get_time();
        let tsc_post = read_timestamp_counter_end();
        let clock_post = clock.get_time_and_bound_and_status();
        let phc_ceb = self.ceb_reader.read();

        let tsc_pre = TscCount::new(tsc_pre as i64);
        let tsc_post = TscCount::new(tsc_post as i64);

        let clock_pre = Self::build_response_from_clock_bound_client_read(clock_pre);
        let ref_clock = Self::build_response_from_phc_read(phc_time, phc_ceb);
        let clock_post = Self::build_response_from_clock_bound_client_read(clock_post);

        let exchange_metadata = if clock_pre.is_err() || ref_clock.is_err() || clock_post.is_err() {
            None
        } else {
            let mid = clock_pre
                .clone()
                .unwrap()
                .time
                .midpoint(clock_post.clone().unwrap().time);
            let offset = mid - ref_clock.clone().unwrap().time;
            let rtt_clock = clock_post.clone().unwrap().time - clock_pre.clone().unwrap().time;
            let rtt_tsc = tsc_post - tsc_pre;

            Some(ExchangeMetadata {
                offset,
                rtt_clock,
                rtt_tsc,
            })
        };

        ClockBoundClientRefComparison {
            clock_pre,
            tsc_pre,
            ref_clock,
            exchange_metadata,
            tsc_post,
            clock_post,
        }
    }
}
