use anyhow::Result;
use std::{fs::File, path::PathBuf};

use clock_bound::daemon::time::inner::{ClockOffsetAndRtt, Time};
use clock_bound::daemon::time::instant::Utc;
use clock_bound::daemon::time::{Clock, ClockExt, Duration};

mod ptp;
use ptp::PtpReader;

mod ceb;
use ceb::PhcClockErrorBoundReader;

mod autoconfiguration;
pub use autoconfiguration::autoconfigure_phc_reader;

#[allow(dead_code)]
#[derive(Debug)]
pub struct OffsetRttAndCeb {
    offset_and_rtt: ClockOffsetAndRtt<Utc>,
    ceb: Duration,
}

#[derive(Debug)]
pub struct PhcReader {
    // we need to hold the actual File to keep the raw fd valid.
    _phc_device_file: File,
    ptp_reader: PtpReader,
    ceb_reader: PhcClockErrorBoundReader,
}
impl PhcReader {
    pub fn new(phc_device_file: File, phc_clock_error_bound_path: PathBuf) -> Self {
        let ptp_reader = PtpReader::new(&phc_device_file);
        let ceb_reader = PhcClockErrorBoundReader::new(phc_clock_error_bound_path);
        Self {
            _phc_device_file: phc_device_file,
            ptp_reader,
            ceb_reader,
        }
    }

    /// Return offset, rtt, and CEB from a clock to PHC comparision.
    ///
    /// # Errors
    /// Returns an error if CEB read operation fails.
    pub fn get_offset_from_utc_clock<T: ClockExt<Utc>>(
        &self,
        clock: &T,
    ) -> Result<OffsetRttAndCeb> {
        let offset_and_rtt = clock.get_offset_and_rtt(self);
        let ceb = self.ceb_reader.read()?;
        Ok(OffsetRttAndCeb {
            offset_and_rtt,
            ceb,
        })
    }
}

/// # Panics
/// Panics if PTP get time system call fails.
/// FIXME: ideally this trait signature would return a Result so we can avoid the panic here.
/// But that would require changing the logic in the daemon libary.
/// We could do that or re-implement this trait here.
impl Clock<Utc> for PhcReader {
    #[doc = " Read the current clock time."]
    fn get_time(&self) -> Time<Utc> {
        self.ptp_reader
            .ptp_get_time()
            .expect("failed to get time from PHC")
    }
}
