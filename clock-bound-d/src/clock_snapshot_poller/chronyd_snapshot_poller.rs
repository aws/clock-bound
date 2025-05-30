use chrony_candm::reply::Tracking;
use nix::sys::time::TimeSpec;
use std::time::{Duration, Instant, SystemTimeError};
use tracing::{error, warn};

#[cfg(any(test, feature = "test"))]
use crate::phc_utils::MockPhcWithSysfsErrorBound as PhcWithSysfsErrorBound;
#[cfg(not(any(test, feature = "test")))]
use crate::phc_utils::PhcWithSysfsErrorBound;
use crate::{chrony_client::ChronyClientExt, ChronyClockStatus};

use super::{ClockStatusSnapshot, ClockStatusSnapshotPoller};

/// Struct implementing ClockSyncInfoPoller which polls Chronyd and adds in
/// ENA PHC error bound data when syncing to an ENA PHC reference clock corresponding to maybe_phc_info,
/// if PhcInfo is supplied.
pub struct ChronyDaemonSnapshotPoller {
    chrony_client: Box<dyn ChronyClientExt>,
    maybe_phc_error_bound_reader: Option<PhcWithSysfsErrorBound>,
}

impl ChronyDaemonSnapshotPoller {
    pub fn new(
        chrony_client: Box<dyn ChronyClientExt>,
        maybe_phc_error_bound_reader: Option<PhcWithSysfsErrorBound>,
    ) -> Self {
        Self {
            chrony_client,
            maybe_phc_error_bound_reader,
        }
    }
}

impl ClockStatusSnapshotPoller for ChronyDaemonSnapshotPoller {
    fn retrieve_clock_status_snapshot(
        &self,
        as_of: TimeSpec,
    ) -> anyhow::Result<ClockStatusSnapshot> {
        let tracking_request_start_time = Instant::now();
        let tracking = self.chrony_client.query_tracking()?;
        let get_tracking_duration = tracking_request_start_time.elapsed();
        if get_tracking_duration.as_millis() > 2000 {
            warn!(
                "Chronyd tracking query took a long time.  Duration: {:?}",
                get_tracking_duration
            );
        }
        let phc_error_bound_nsec = match &self.maybe_phc_error_bound_reader {
            // Only add PHC error bound if PHC info was supplied via CLI and
            // current tracking reference uses it.
            Some(phc_error_bound_reader)
                if phc_error_bound_reader.get_phc_ref_id() == tracking.ref_id =>
            {
                match phc_error_bound_reader.read_phc_error_bound() {
                    Ok(phc_error_bound) => phc_error_bound,
                    Err(e) => {
                        anyhow::bail!("Failed to retrieve PHC error bound: {:?}", e);
                    }
                }
            }
            _ => 0,
        };
        let error_bound_nsec = tracking.extract_error_bound_nsec() + phc_error_bound_nsec;
        let chrony_clock_status = tracking.get_chrony_clock_status()?;
        Ok(ClockStatusSnapshot {
            error_bound_nsec,
            chrony_clock_status,
            as_of,
        })
    }
}

#[cfg_attr(any(test, feature = "test"), mockall::automock)]
trait TrackingExt {
    fn extract_error_bound_nsec(&self) -> i64;
    fn get_chrony_clock_status(&self) -> anyhow::Result<ChronyClockStatus, SystemTimeError>;
}

impl TrackingExt for Tracking {
    fn extract_error_bound_nsec(&self) -> i64 {
        let root_delay: f64 = self.root_delay.into();
        let root_dispersion: f64 = self.root_dispersion.into();
        let current_correction: f64 = f64::from(self.current_correction).abs();

        // Compute the clock error bound in nanoseconds *at the time chrony reported the tracking data*.
        // Remember that the root dispersion reported by chrony is at the time the tracking data is
        // retrieved, not at the time of the last system clock update.
        ((root_delay / 2. + root_dispersion + current_correction) * 1_000_000_000.0).ceil() as i64
    }

    fn get_chrony_clock_status(&self) -> anyhow::Result<ChronyClockStatus, SystemTimeError> {
        // Compute the duration since the last time chronyd updated the system clock.
        let duration_since_update = self.ref_time.elapsed().inspect_err(|e| {
            error!(
                error = ?e,
                "Failed to get duration since chronyd last clock update",
            );
        })?;

        // Compute the time it would take for chronyd 8-wide register to be completely empty (e.g. the
        // last 8 NTP requests timed out)
        let polling_period = f64::from(self.last_update_interval);
        let empty_register_timeout = Duration::from_secs((polling_period * 8.0) as u64);

        // Get the status reported by chrony and tracking data.
        // Chronyd tends to report a synchronized status for a very looooong time after it has failed
        // to continuously receive NTP responses. Here the status is over-written if the last time
        // chronyd successfully updated the system clock is "too old". Too old is define as the time it
        // would take for the 8-wide register to become empty.
        let chrony_clock_status = match ChronyClockStatus::from(self.leap_status) {
            ChronyClockStatus::Synchronized => {
                if duration_since_update > empty_register_timeout {
                    ChronyClockStatus::FreeRunning
                } else {
                    ChronyClockStatus::Synchronized
                }
            }
            status => status,
        };
        Ok(chrony_clock_status)
    }
}

#[cfg(test)]
mod test_chrony_daemon_snapshot_poller {
    use super::*;

    use crate::chrony_client::MockChronyClientExt;
    use crate::{phc_utils::MockPhcWithSysfsErrorBound, ChronyClockStatus};

    use chrony_candm::common::ChronyFloat;
    use chrony_candm::{common::ChronyAddr, reply::Tracking};
    use rstest::rstest;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(bon::Builder)]
    struct TrackingBuilder {
        #[builder(default)]
        pub ref_id: u32,
        #[builder(default)]
        pub ip_addr: ChronyAddr,
        #[builder(default)]
        pub stratum: u16,
        #[builder(default)]
        pub leap_status: u16,
        #[builder(default = UNIX_EPOCH)]
        pub ref_time: SystemTime,
        #[builder(default)]
        pub current_correction: ChronyFloat,
        #[builder(default)]
        pub last_offset: ChronyFloat,
        #[builder(default)]
        pub rms_offset: ChronyFloat,
        #[builder(default)]
        pub freq_ppm: ChronyFloat,
        #[builder(default)]
        pub resid_freq_ppm: ChronyFloat,
        #[builder(default)]
        pub skew_ppm: ChronyFloat,
        #[builder(default)]
        pub root_delay: ChronyFloat,
        #[builder(default)]
        pub root_dispersion: ChronyFloat,
        #[builder(default)]
        pub last_update_interval: ChronyFloat,
    }

    impl Into<Tracking> for TrackingBuilder {
        fn into(self) -> Tracking {
            Tracking {
                ref_id: self.ref_id,
                ip_addr: self.ip_addr,
                stratum: self.stratum,
                leap_status: self.leap_status,
                ref_time: self.ref_time,
                current_correction: self.current_correction,
                last_offset: self.last_offset,
                rms_offset: self.rms_offset,
                freq_ppm: self.freq_ppm,
                resid_freq_ppm: self.resid_freq_ppm,
                skew_ppm: self.skew_ppm,
                root_delay: self.root_delay,
                root_dispersion: self.root_dispersion,
                last_update_interval: self.last_update_interval,
            }
        }
    }

    #[rstest]
    #[case::query_tracking_failed(
        Err(anyhow::anyhow!("Some error")),
        None,
        0,
        Ok(0),
    )]
    #[case::get_phc_error_bound_failed(
        Ok(TrackingBuilder::builder().ref_id(123).build().into()),
        Some(MockPhcWithSysfsErrorBound::default()),
        123,
        Err(anyhow::anyhow!("Some error")),
    )]
    #[case::get_chrony_clock_status_failed(
        Ok(
            // Invalid tracking should fail get_chrony_clock_status
            TrackingBuilder::builder()
                .ref_time(SystemTime::now() + Duration::from_secs(123))
                .build()
                .into()
        ),
        None,
        0,
        Ok(0),
    )]
    fn test_retrieve_clock_status_snapshot_failure(
        #[case] tracking_return_val: anyhow::Result<Tracking>,
        #[case] mut maybe_phc_error_bound_reader: Option<MockPhcWithSysfsErrorBound>,
        #[case] phc_ref_id_return_val: u32,
        #[case] phc_error_bound_return_val: anyhow::Result<i64>,
    ) {
        // We only ever expect the PHC error bound to be read if both are supplied
        // and tracking ref ID == PHC ref ID
        if let (Ok(tracking), Some(phc_error_bound_reader)) =
            (&tracking_return_val, &mut maybe_phc_error_bound_reader)
        {
            let mut sequence = mockall::Sequence::new();
            phc_error_bound_reader
                .expect_get_phc_ref_id()
                .once()
                .return_once(move || phc_ref_id_return_val)
                .in_sequence(&mut sequence);
            if phc_ref_id_return_val == tracking.ref_id {
                phc_error_bound_reader
                    .expect_read_phc_error_bound()
                    .once()
                    .return_once(move || phc_error_bound_return_val)
                    .in_sequence(&mut sequence);
            }
        }

        let mut mock_chrony_client = Box::new(MockChronyClientExt::new());
        mock_chrony_client
            .expect_query_tracking()
            .once()
            .return_once(|| tracking_return_val);
        let poller =
            ChronyDaemonSnapshotPoller::new(mock_chrony_client, maybe_phc_error_bound_reader);
        let rt = poller.retrieve_clock_status_snapshot(TimeSpec::new(123, 456));
        assert!(rt.is_err());
    }

    #[rstest]
    #[case::with_phc_ref_id_matching_tracking_ref_id(
        Some(MockPhcWithSysfsErrorBound::default()),
        TrackingBuilder::builder()
            .ref_id(123)
            .ref_time(SystemTime::now())
            .last_update_interval(1.0.into())
            .current_correction((-1.0).into())
            .root_dispersion(2.0.into())
            .root_delay(1.0.into())
            .build().into(),
        123,
        123456,
        3_500_123_456,
        ChronyClockStatus::Synchronized,
    )]
    #[case::with_phc_info_not_matching_ref_id(
        Some(MockPhcWithSysfsErrorBound::default()),
        TrackingBuilder::builder()
            .ref_id(123)
            .ref_time(SystemTime::now())
            .last_update_interval(1.0.into())
            .current_correction((-1.0).into())
            .root_dispersion(2.0.into())
            .root_delay(1.0.into())
            .build().into(),
        234,
        123456,
        3_500_000_000,
        ChronyClockStatus::Synchronized,
    )]
    #[case::with_no_phc_info(
        None,
        TrackingBuilder::builder()
            .ref_id(123)
            .ref_time(SystemTime::now())
            .last_update_interval(1.0.into())
            .current_correction((-1.0).into())
            .root_dispersion(2.0.into())
            .root_delay(1.0.into())
            .build().into(),
        123,
        123456,
        3_500_000_000,
        ChronyClockStatus::Synchronized
    )]
    #[case::chrony_is_freerunning(
        None,
        TrackingBuilder::builder()
            .leap_status(3)
            .ref_id(123)
            .ref_time(SystemTime::now())
            .last_update_interval(1.0.into())
            .current_correction((-1.0).into())
            .root_dispersion(2.0.into())
            .root_delay(1.0.into())
            .build().into(),
        234,
        123456,
        3_500_000_000,
        ChronyClockStatus::FreeRunning,
    )]
    fn test_retrieve_clock_status_snapshot_success_synchronized(
        #[case] mut maybe_phc_error_bound_reader: Option<MockPhcWithSysfsErrorBound>,
        #[case] tracking_return_val: Tracking,
        #[case] phc_ref_id_return_val: u32,
        #[case] phc_error_bound_return_val: i64,
        #[case] expected_bound_nsec: i64,
        #[case] expected_chrony_clock_status: ChronyClockStatus,
    ) {
        if let Some(phc_error_bound_reader) = &mut maybe_phc_error_bound_reader {
            let mut sequence = mockall::Sequence::new();
            phc_error_bound_reader
                .expect_get_phc_ref_id()
                .once()
                .return_once(move || phc_ref_id_return_val)
                .in_sequence(&mut sequence);
            if phc_ref_id_return_val == tracking_return_val.ref_id {
                phc_error_bound_reader
                    .expect_read_phc_error_bound()
                    .once()
                    .return_once(move || Ok(phc_error_bound_return_val))
                    .in_sequence(&mut sequence);
            }
        }
        let mut mock_chrony_client = Box::new(MockChronyClientExt::new());
        mock_chrony_client
            .expect_query_tracking()
            .once()
            .return_once(move || Ok(tracking_return_val));
        let poller =
            ChronyDaemonSnapshotPoller::new(mock_chrony_client, maybe_phc_error_bound_reader);
        let rt = poller.retrieve_clock_status_snapshot(TimeSpec::new(123, 456));
        assert!(rt.is_ok());
        let rt = rt.unwrap();
        assert_eq!(rt.chrony_clock_status, expected_chrony_clock_status);
        assert_eq!(rt.error_bound_nsec, expected_bound_nsec);
    }

    /// Assert that clock error bound is calculated properly from current_correction, root_delay, root_dispersion
    /// in both positive and negative current_correction cases.
    #[test]
    fn test_extract_error_bound_nsec_from_tracking() {
        let mut tracking: Tracking = TrackingBuilder::builder()
            .current_correction(1.0.into()) // -1 second offset, contributes 1 second to error bound
            .root_delay(3.0.into()) // 3 second root delay (contributes 3 / 2 = 1.5 seconds to error bound)
            .root_dispersion(2.0.into()) // 2 second root dispersion, contributes 2 seconds to error bound
            .build()
            .into();
        let error_bound_nsec = tracking.extract_error_bound_nsec();
        assert_eq!(error_bound_nsec, 4_500_000_000);
        // validate negative case too, should still contribute 1 second to error bound
        tracking.current_correction = (-1.0).into();
        let error_bound_nsec = tracking.extract_error_bound_nsec();
        assert_eq!(error_bound_nsec, 4_500_000_000);
    }

    #[rstest]
    #[case::synchronized_and_ref_time_within_8_polls(
        TrackingBuilder::builder().last_update_interval(2.0.into()).leap_status(0).ref_time(SystemTime::now()).build().into(),
        ChronyClockStatus::Synchronized
    )]
    #[case::synchronized_but_ref_time_more_than_8_polls_ago(
        TrackingBuilder::builder().last_update_interval(2.0.into()).leap_status(0).ref_time(UNIX_EPOCH).build().into(),
        ChronyClockStatus::FreeRunning
    )]
    #[case::leap_status_unsynchronized(
        TrackingBuilder::builder().last_update_interval(2.0.into()).leap_status(3).ref_time(SystemTime::now()).build().into(),
        ChronyClockStatus::FreeRunning
    )]
    #[case::leap_status_invalid(
        TrackingBuilder::builder().last_update_interval(2.0.into()).leap_status(4).ref_time(SystemTime::now()).build().into(),
        ChronyClockStatus::Unknown
    )]
    fn test_get_chrony_clock_status_success(
        #[case] tracking: Tracking,
        #[case] expected_chrony_clock_status: ChronyClockStatus,
    ) {
        let rt = tracking.get_chrony_clock_status();
        assert!(rt.is_ok());
        assert_eq!(rt.unwrap(), expected_chrony_clock_status);
    }

    #[test]
    fn test_get_chrony_clock_status_failure() {
        // Set the time in the future, which causes us to fail to determine the current time.
        let tracking: Tracking = TrackingBuilder::builder()
            .ref_time(SystemTime::now() + Duration::from_secs(123))
            .build()
            .into();
        let rt = tracking.get_chrony_clock_status();
        assert!(rt.is_err());
    }
}
