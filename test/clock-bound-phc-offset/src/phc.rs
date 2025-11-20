use anyhow::Result;
use clock_bound::daemon::io::tsc::{read_timestamp_counter_begin, read_timestamp_counter_end};
pub use clock_bound_client_generic::{ClockBoundClientClock, TimeAndBoundAndStatus};
use std::{fs::File, path::PathBuf};

use clock_bound::daemon::time::instant::Utc;
use clock_bound::daemon::time::{ClockExt, Duration, Instant, TscCount, TscDiff};

mod ptp;
use ptp::PtpReader;

mod ceb;
use ceb::PhcClockErrorBoundReader;

mod autoconfiguration;
pub use autoconfiguration::autoconfigure_phc_reader;

/// Convenience struct holding data pertaining to a snaphot from a reference PHC read.
#[allow(dead_code)]
#[derive(Debug, serde::Serialize)]
struct RefClockData {
    /// Timestamp read from PHC.
    time: Instant,
    /// Offset between midpoint of pre/post clock read and PHC timestamp.
    /// A positive value here indicates that the clock is ahead of the PHC.
    offset: Duration,
    /// RTT representing duration between pre/post clock reads (and bounding PHC read).
    /// Period is inherited from the clock we're reading from.
    rtt_clock: Duration,
    /// RTT in representing duration between pre/post TSC reads (and bounding PHC read).
    /// Value represents elapsed TSC ticks.
    rtt_tsc: TscDiff,
    /// Clock Error Bound reported by the PHC.
    /// This is read just after the post clock read operation.
    ceb: Duration,
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
    ref_clock: RefClockData,
    tsc_post: TscCount,
    clock_post: BasicClockTime,
}

/// Convenience struct representing all data of interest from a comparison read between a clock bound client clock and the PHC.
#[allow(dead_code)]
#[derive(Debug, serde::Serialize)]
pub struct ClockBoundClientRefComparison {
    clock_pre: TimeAndBoundAndStatus,
    tsc_pre: TscCount,
    ref_clock: RefClockData,
    tsc_post: TscCount,
    clock_post: TimeAndBoundAndStatus,
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
    pub fn get_time(&self) -> Result<Instant> {
        self.ptp_reader.ptp_get_time()
    }

    /// Return details from a "basic" UTC clock to PHC comparision.
    ///
    /// # Errors
    /// Returns an error if the PHC read or CEB read operation fails.
    pub fn get_offset_from_utc_clock<T: ClockExt<Utc>>(
        &self,
        clock: &T,
    ) -> Result<BasicClockRefComparison> {
        let clock_pre = clock.get_time();
        let tsc_pre = read_timestamp_counter_begin();
        let phc_time = self.get_time()?;
        let tsc_post = read_timestamp_counter_end();
        let clock_post = clock.get_time();
        let phc_ceb = self.ceb_reader.read()?;

        let mid = clock_pre.midpoint(clock_post);
        let offset = mid - phc_time;
        let rtt_clock = clock_post - clock_pre;

        let tsc_pre = TscCount::new(tsc_pre.into());
        let tsc_post = TscCount::new(tsc_post.into());
        let rtt_tsc = tsc_post - tsc_pre;

        Ok(BasicClockRefComparison {
            clock_pre: BasicClockTime::from(clock_pre),
            tsc_pre,
            ref_clock: RefClockData {
                time: phc_time,
                offset,
                rtt_clock,
                rtt_tsc,
                ceb: phc_ceb,
            },
            tsc_post,
            clock_post: BasicClockTime::from(clock_post),
        })
    }

    /// Return comparison data from a ClockBound client clock to PHC.
    ///
    /// # Errors
    /// Returns an error if the PTP syscall, PHC read, or client read operations fail.
    pub fn compare_to_clock_bound_client_clock<T: ClockBoundClientClock>(
        &self,
        clock: &mut T,
    ) -> Result<ClockBoundClientRefComparison> {
        let clock_pre = clock.get_time_and_bound_and_status()?;
        let tsc_pre = read_timestamp_counter_begin();
        let phc_time = self.get_time()?;
        let tsc_post = read_timestamp_counter_end();
        let clock_post = clock.get_time_and_bound_and_status()?;
        let phc_ceb = self.ceb_reader.read()?;

        let mid = clock_pre.time.midpoint(clock_post.time);
        let offset = mid - phc_time;
        let rtt_clock = clock_post.time - clock_pre.time;

        let tsc_pre = TscCount::new(tsc_pre.into());
        let tsc_post = TscCount::new(tsc_post.into());
        let rtt_tsc = tsc_post - tsc_pre;

        Ok(ClockBoundClientRefComparison {
            clock_pre,
            tsc_pre,
            ref_clock: RefClockData {
                time: phc_time,
                offset,
                rtt_clock,
                rtt_tsc,
                ceb: phc_ceb,
            },
            tsc_post,
            clock_post,
        })
    }
}
