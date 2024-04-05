// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only

use clock_bound_shm::common::{clock_gettime_safe, CLOCK_MONOTONIC};
use chrony_candm::blocking_query_uds;
use chrony_candm::reply::{ReplyBody, Tracking};
use chrony_candm::request::RequestBody;
use chrony_candm::ClientOptions;
use std::io::Read;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

use crate::thread_manager::Context;
use crate::{ChannelId, Message, PhcInfo};

/// The chronyd daemon may be restarted from time to time. This may not necessarily implies that
/// the clock error should not be trusted. This constant defines the amount of time the clock can
/// be kept in FREE_RUNNING mode, before moving to UNKNOWN.
const CHRONY_RESTART_GRACE_PERIOD: Duration = Duration::from_secs(5);

/// Trait specific to this module to ease unit testing
trait ChronyOperations {
    /// Request tracking information from chronyd. Take a mutable reference to self to allow for
    /// unit test side effects.
    fn get_tracking(&mut self) -> Option<Tracking>;

    /// Return True if the last tracking data from chronyd is within the grace period.
    fn is_within_grace_period(&self) -> bool;
}

/// A struct to use for polling clock error bound data.
struct ClockErrorBoundPoller {
    /// The instant tracking the last time chrony tracking data was successfully received
    last_tracking_data: Instant,
}

impl Default for ClockErrorBoundPoller {
    /// Build a ClockErrorBoundPoller, setting last_tracking_data in the past, beyond the grace period.
    fn default() -> Self {
        Self {
            last_tracking_data: Instant::now()
                .checked_sub(CHRONY_RESTART_GRACE_PERIOD)
                .unwrap(),
        }
    }
}

impl ChronyOperations for ClockErrorBoundPoller {
    /// Request tracking information from chronyd over a Unix Datagram Socket.
    ///
    /// Getting Tracking data is a read-only operation. The chronyd daemon accepts these operations
    /// over both a UDS as well as a UDP socket over the IPv4/IPv6 loopback addresses by default.
    /// Here we prefer a local UDS socket because it allows for mutable operations, which we
    /// may utilize in the future.
    ///
    /// The use of a UDS socket brings all sorts of permission issues. In particular, if chronyd
    /// runs as the "chrony" user, chronyd sets the permissions on the UDS to the "chrony" user
    /// only. Even if the UDS socket is not strictly required here, we use it to have an early
    /// and periodic signal that things are off and we may have a permissions problem.
    fn get_tracking(&mut self) -> Option<Tracking> {
        // Request chronyd Tracking data over a local Unix Datagram Socket.
        let request_body = RequestBody::Tracking;

        // The Default ClientOptions have a timeout of 1 second and attempts 3 retries. However,
        // this may not apply to the UDS socket if chronyd has terminated and deleted the socket
        // file. In this case, the timeout is ignored since the destination is not reachable and
        // call will return almost right away.
        let options = ClientOptions::default();

        match blocking_query_uds(request_body, options) {
            Err(e) => {
                error!("No reply from chronyd. Is it running? Error: {:?}", e);
                None
            }
            Ok(reply) => {
                if let ReplyBody::Tracking(body) = reply.body {
                    debug!("Received chronyd Tracking data");
                    self.last_tracking_data = Instant::now();
                    Some(body)
                } else {
                    error!(
                        "Reply from chronyd was invalid. Expected tracking data but got: {:?}",
                        reply
                    );
                    None
                }
            }
        }
    }

    /// Determine if the grace period has expired or not.
    ///
    /// Returns True if within the grace period, False otherwise.
    fn is_within_grace_period(&self) -> bool {
        self.last_tracking_data.elapsed() < CHRONY_RESTART_GRACE_PERIOD
    }
}

/// Continuously poll for clock error bound data from chronyd and PHC interface if supplied.
///
/// Once successfully retrieved, the chronyd tracking data is sent over to the thread writing the
/// clock error bound to the shared memory segment. Note that the scheduling of requests is based
/// on expiring a timeout on the thread mailbox. This is equivalent to a `sleep()` and will wait
/// for *at least* the duration passed in `sleep`.
fn run_clock_error_bound_poller(
    ctx: Context,
    mut poller: impl ChronyOperations,
    phc_info: Option<PhcInfo>,
    sleep: Duration,
) {
    let mut keep_running = true;

    // Keep on running forever until we receive the instruction to stop.
    while keep_running {
        // First, make sure we take a MONOTONIC timestamp *before* getting chronyd data. This will
        // slightly inflate the dispersion component of the clock error bound but better be
        // pessimistic and correct, than greedy and wrong. The actual error added here is expected
        // to be small. For example, let's assume a 50PPM drift rate. Let's also assume it takes 10
        // milliseconds for chronyd to respond. This will inflate the CEB by 500 nanoseconds.
        //
        // TODO: implement a retry strategy to filter out calls to chronyd that would have been
        // scheduled out or delayed.
        match clock_gettime_safe(CLOCK_MONOTONIC) {
            Ok(as_of) => {
                // If polling is successful, pass the tracking data and monotonic timestamp to the
                // shm writer. Otherwise signal chrony is not responding.
                let message = match poller.get_tracking() {
                    Some(tracking) => match &phc_info {
                        Some(phc_info) if phc_info.refid == tracking.ref_id => {
                            match get_phc_error_bound_from_path(&phc_info.sysfs_error_bound_path) {
                                Ok(phc_error_bound) => {
                                    Message::ClockErrorBoundData((tracking, phc_error_bound, as_of))
                                }
                                Err(e) => {
                                    error!("Failed to retrieve PHC error bound: {:?}", e);
                                    if poller.is_within_grace_period() {
                                        Message::PhcErrorBoundRetrievalFailedGracePeriod
                                    } else {
                                        Message::PhcErrorBoundRetrievalFailed
                                    }
                                }
                            }
                        }
                        _ => Message::ClockErrorBoundData((tracking, 0, as_of)),
                    },
                    None => {
                        // TODO: this may be best implemented within the SHM Writer state machine.
                        if poller.is_within_grace_period() {
                            Message::ChronyNotRespondingGracePeriod
                        } else {
                            Message::ChronyNotResponding
                        }
                    }
                };

                match ctx.dbox.send(&ChannelId::ShmWriter, message) {
                    Ok(()) => (),
                    Err(_) => {
                        error!("The receiving end of the ShmWriter MPSC channel is disconnected");
                        panic!("Broken channel to ShmWriter");
                    }
                };
            }
            Err(e) => error!(
                "Failed to retrieve monotonic clock time before polling chronyd {:?}",
                e
            ),
        }

        // TODO: this is a very naive implementation. If messages are received in a burst, this
        // would hit chronyd at the same pace. In the current implementation, this is not happening
        // since only the Abort message is meant to be sent to the chronyd polling thread. However,
        // should improve on this to make it robust by having a more dynamic sleep time.
        match ctx.mbox.recv_timeout(sleep) {
            Ok(Message::ThreadAbort) => {
                info!("Received message to stop polling chronyd");
                keep_running = false;
            }
            Ok(msg) => info!("Received unexpected message {:?}", msg),
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(e) => error!("Error reading from MPSC channel: {:?}", e),
        }
    }
}

/// Entry point to this thread.
pub fn run(ctx: Context, phc_info: Option<PhcInfo>) {
    info!("Starting chronyd polling thread");
    let poller = ClockErrorBoundPoller::default();
    let sleep = Duration::from_millis(1000);
    run_clock_error_bound_poller(ctx, poller, phc_info, sleep);
}

/// Reads the PHC Error Bound and parses to a float.
///
/// # Arguments
///
/// * `phc_error_bound_path` - The path of sysfs file to read PHC error bound from.
fn get_phc_error_bound_from_path(
    phc_error_bound_path: &std::path::Path,
) -> Result<i64, std::io::Error> {
    let mut contents = String::new();
    std::fs::File::open(phc_error_bound_path)?.read_to_string(&mut contents)?;
    Ok(contents
        .trim()
        .parse::<i64>()
        .expect("Could not parse error bound value to i64"))
}

#[cfg(test)]
mod t_chrony_poller {
    use chrony_candm::common::ChronyAddr;
    use chrony_candm::reply::Tracking;
    use std::collections::VecDeque;
    use std::io::Write;
    use std::sync::mpsc::Receiver;
    use std::time::SystemTime;

    use super::*;
    use crate::channels::new_channel_web;

    struct TestClockErrorBoundPoller {
        side_effect: VecDeque<Option<Tracking>>,
        is_within_grace_period: bool,
    }

    impl TestClockErrorBoundPoller {
        fn new(
            side_effect: Vec<Option<Tracking>>,
            is_within_grace_period: bool,
        ) -> TestClockErrorBoundPoller {
            TestClockErrorBoundPoller {
                side_effect: side_effect.into(),
                is_within_grace_period,
            }
        }
    }

    impl ChronyOperations for TestClockErrorBoundPoller {
        fn get_tracking(&mut self) -> Option<Tracking> {
            self.side_effect.pop_front().unwrap()
        }

        fn is_within_grace_period(&self) -> bool {
            self.is_within_grace_period
        }
    }

    /// Store test resources in a struct so that they are always cleaned up on test pass and fail
    struct TestResources {
        pub path: std::path::PathBuf,
    }

    impl TestResources {
        fn new(p: &str) -> Self {
            let file_path = std::path::PathBuf::from(p);
            std::fs::create_dir_all(&file_path).expect("Failed to create test log directory");
            TestResources { path: file_path }
        }
    }

    impl Drop for TestResources {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.path).expect("Failed to remove testing directory");
        }
    }

    fn setup_channels() -> (Context, Receiver<Message>) {
        let (mut mbox, dbox) =
            new_channel_web(vec![ChannelId::ClockErrorBoundPoller, ChannelId::ShmWriter]);
        let shm_mailbox = mbox.get_mailbox(&ChannelId::ShmWriter).unwrap();
        let mbox = mbox.get_mailbox(&ChannelId::ClockErrorBoundPoller).unwrap();
        let ctx = Context {
            mbox,
            dbox,
            channel_id: ChannelId::ClockErrorBoundPoller,
        };
        (ctx, shm_mailbox)
    }

    /// Helper for sending the Context filler messages so that we don't actually wait
    /// the full recv_timeout for each iteration of our running loop.
    fn send_context_filler_messages(ctx: &Context, iterations: usize) {
        // Fill a few (bogus) messages until asking to abort. This defines the number of iterations
        // in the loop.
        for _ in 0..iterations - 1 {
            let _ = ctx.dbox.send(
                &ChannelId::ClockErrorBoundPoller,
                Message::ChronyNotRespondingGracePeriod,
            );
        }
        let _ = ctx
            .dbox
            .send(&ChannelId::ClockErrorBoundPoller, Message::ThreadAbort);
    }

    fn build_tracking() -> Tracking {
        Tracking {
            ref_id: 0,
            ip_addr: ChronyAddr::default(),
            stratum: 1,
            leap_status: 1,
            ref_time: SystemTime::now(),
            current_correction: 0.0.into(),
            last_offset: 0.0.into(),
            rms_offset: 0.0.into(),
            freq_ppm: 0.0.into(),
            resid_freq_ppm: 0.0.into(),
            skew_ppm: 0.0.into(),
            root_delay: 0.0.into(),
            root_dispersion: 0.0.into(),
            last_update_interval: 0.0.into(),
        }
    }

    /// Assert that a ChronyNotResponsiveGracePeriod message is sent when chrony does not return
    /// tracking data but within the grace period.
    #[test]
    fn test_message_if_chrony_unresponsive_within_grace_period() {
        // Build the context to pass over and the mailbox representing the ShmWriter thread.
        let (ctx, mbox_shm) = setup_channels();
        send_context_filler_messages(&ctx, 3);
        // Mock chrony poller. Set things up such that no chrony tracking data can be found in any
        // of the loop iteration.
        let side_effect = vec![None, None, None];
        let poller = TestClockErrorBoundPoller::new(side_effect, true);

        // Let it run, sleep very little time
        run_clock_error_bound_poller(ctx, poller, None, Duration::from_millis(1));

        // There should have been exactly three messages sent to the SHM Writer
        let mut iter = mbox_shm.iter();
        assert_eq!(iter.next(), Some(Message::ChronyNotRespondingGracePeriod));
        assert_eq!(iter.next(), Some(Message::ChronyNotRespondingGracePeriod));
        assert_eq!(iter.next(), Some(Message::ChronyNotRespondingGracePeriod));
        assert_eq!(iter.next(), None);
    }

    /// Assert that a ChronyNotResponsive message is sent when chrony does not return tracking data
    /// beyond the grace period.
    #[test]
    fn test_message_if_chrony_unresponsive() {
        // Build the context to pass over and the mailbox representing the ShmWriter thread.
        let (ctx, mbox_shm) = setup_channels();
        send_context_filler_messages(&ctx, 3);

        // Mock chrony poller. Set things up such that no chrony tracking data can be found in any
        // of the loop iteration.
        let side_effect = vec![None, None, None];
        let poller = TestClockErrorBoundPoller::new(side_effect, false);

        // Let it run, sleep very little time
        run_clock_error_bound_poller(ctx, poller, None, Duration::from_millis(1));

        // There should have been exactly three messages sent to the SHM Writer
        let mut iter = mbox_shm.iter();
        assert_eq!(iter.next(), Some(Message::ChronyNotResponding));
        assert_eq!(iter.next(), Some(Message::ChronyNotResponding));
        assert_eq!(iter.next(), Some(Message::ChronyNotResponding));
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn test_message_phc_error_bound_retrieved_happy_paths() {
        // Mock the error bound file with contents being 12345.
        let test_resources =
            TestResources::new("test_message_phc_error_bound_retrieved_happy_path");
        let sysfs_error_bound_path = test_resources
            .path
            .join(std::path::PathBuf::from("phc_error_bound"));
        std::fs::File::create(&sysfs_error_bound_path)
            .expect("Failed to create PHC error bound file")
            .write_all("12345".as_bytes())
            .expect("Failed to write PHC error bound to file");
        // Build the context to pass over and the mailbox representing the ShmWriter thread.
        let (ctx, mbox_shm) = setup_channels();
        send_context_filler_messages(&ctx, 3);

        let tracking = build_tracking();
        // Set ref_id to not be equivalent to the PhcInfo provided so we don't bother grabbing the error bound
        // from sysfs and return 0 instead
        let mut second_tracking = build_tracking();
        second_tracking.ref_id = 1;
        // Mock chrony poller. Set things up such that we:
        // 1. Receive tracking with ref id 0, same as phc ID, so we receive PHC error bound
        // 2. No tracking data, so ChronyNotRespondingGracePeriod
        // 3. Receive tracking with ref id 1, not same as phc ID, so PHC error bound defaults to 0
        let side_effect = vec![Some(tracking), None, Some(second_tracking)];
        let poller = TestClockErrorBoundPoller::new(side_effect, true);

        // Let it run, sleep very little time. PHC info has the same ref ID as our default tracking so we try
        // to access, but accessing a nonexistent path will result in a failure.
        run_clock_error_bound_poller(
            ctx,
            poller,
            Some(PhcInfo {
                refid: 0,
                sysfs_error_bound_path,
            }),
            Duration::from_millis(1),
        );

        // There should have been exactly three messages sent to the SHM Writer
        let mut iter = mbox_shm.iter();
        // Message 1
        let Message::ClockErrorBoundData((tracking_msg, 12345, _)) = iter.next().unwrap() else {
            panic!("Bad message")
        };
        assert_eq!(tracking_msg, tracking);
        // Message 2
        let Message::ChronyNotRespondingGracePeriod = iter.next().unwrap() else {
            panic!("Bad message")
        };
        // Message 1
        let Message::ClockErrorBoundData((tracking_msg, 0, _)) = iter.next().unwrap() else {
            panic!("Bad message")
        };
        assert_eq!(tracking_msg, second_tracking);

        assert_eq!(iter.next(), None);
    }

    #[test]
    fn test_message_if_phc_error_bound_retrieval_failed_in_grace_period() {
        // Build the context to pass over and the mailbox representing the ShmWriter thread.
        let (ctx, mbox_shm) = setup_channels();
        send_context_filler_messages(&ctx, 3);
        let tracking = build_tracking();
        // Mock chrony poller. Set things up such that we receive Tracking data, but then fail to get the corresponding
        // PHC error bound.
        let side_effect = vec![Some(tracking); 3];
        let poller = TestClockErrorBoundPoller::new(side_effect, true);

        // Let it run, sleep very little time. PHC info has the same ref ID as our default tracking so we try
        // to access, but accessing a nonexistent path will result in a failure.
        run_clock_error_bound_poller(
            ctx,
            poller,
            Some(PhcInfo {
                refid: 0,
                sysfs_error_bound_path: std::path::PathBuf::from("fakepath"),
            }),
            Duration::from_millis(1),
        );

        // There should have been exactly three messages sent to the SHM Writer
        let mut iter = mbox_shm.iter();
        assert_eq!(
            iter.next(),
            Some(Message::PhcErrorBoundRetrievalFailedGracePeriod)
        );
        assert_eq!(
            iter.next(),
            Some(Message::PhcErrorBoundRetrievalFailedGracePeriod)
        );
        assert_eq!(
            iter.next(),
            Some(Message::PhcErrorBoundRetrievalFailedGracePeriod)
        );
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn test_message_if_phc_error_bound_retrieval_failed() {
        // Build the context to pass over and the mailbox representing the ShmWriter thread.
        let (ctx, mbox_shm) = setup_channels();

        // Fill a few (bogus) messages until asking to abort. This defines the number of iterations
        // in the loop.
        send_context_filler_messages(&ctx, 3);

        let tracking = build_tracking();
        // Mock chrony poller. Set things up such that we receive Tracking data, but then fail to get the corresponding
        // PHC error bound.
        let side_effect = vec![Some(tracking); 3];
        let poller = TestClockErrorBoundPoller::new(side_effect, false);

        // Let it run, sleep very little time. PHC info has the same ref ID as our default tracking so we try
        // to access, but accessing a nonexistent path will result in a failure.
        run_clock_error_bound_poller(
            ctx,
            poller,
            Some(PhcInfo {
                refid: 0,
                sysfs_error_bound_path: std::path::PathBuf::from("fakepath"),
            }),
            Duration::from_millis(1),
        );

        // There should have been exactly three messages sent to the SHM Writer
        let mut iter = mbox_shm.iter();
        assert_eq!(iter.next(), Some(Message::PhcErrorBoundRetrievalFailed));
        assert_eq!(iter.next(), Some(Message::PhcErrorBoundRetrievalFailed));
        assert_eq!(iter.next(), Some(Message::PhcErrorBoundRetrievalFailed));
        assert_eq!(iter.next(), None);
    }

    /// Assert that tracking data is sent when chrony is partially responsive
    #[test]
    fn test_tracking_data_delivered() {
        // Build the context to pass over and the mailbox representing the ShmWriter thread.
        let (ctx, mbox_shm) = setup_channels();
        send_context_filler_messages(&ctx, 3);
        // Mock chrony poller. Set things up such that chrony tracking data is found in some iterations and missed in others.
        let tracking = build_tracking();
        let side_effect = vec![Some(tracking), None, Some(tracking)];
        let poller = TestClockErrorBoundPoller::new(side_effect, false);

        // Let it run, sleep very little time
        run_clock_error_bound_poller(ctx, poller, None, Duration::from_millis(1));

        // There should have been exactly one tracking messages, one non-responsive message and
        // another tracking message sent to the ShmWriter
        let mut iter = mbox_shm.iter();

        // Message 1
        let Message::ClockErrorBoundData((tracking_msg, 0, _)) = iter.next().unwrap() else {
            panic!("Bad message")
        };
        assert_eq!(tracking_msg, tracking);

        // Message 2
        assert_eq!(iter.next(), Some(Message::ChronyNotResponding));

        // Message 3
        let Message::ClockErrorBoundData((tracking_msg, 0, _)) = iter.next().unwrap() else {
            panic!("Bad message")
        };
        assert_eq!(tracking_msg, tracking);

        assert_eq!(iter.next(), None);
    }

    #[test]
    fn test_get_phc_error_bound_from_path_happy_path() {
        let phc_error_bound_path = "test_get_phc_error_bound_from_path_happy_path";
        let mut test_phc_error_bound_file = std::fs::File::create(phc_error_bound_path)
            .expect(&format!("Failed to create file {}", phc_error_bound_path));
        test_phc_error_bound_file
            .write_all("12345".as_bytes())
            .expect(&format!("Failed to write to {} file", phc_error_bound_path));
        match get_phc_error_bound_from_path(&std::path::PathBuf::from(phc_error_bound_path)) {
            Ok(v) => assert_eq!(v, 12345),
            Err(e) => {
                std::fs::remove_file(phc_error_bound_path)
                    .expect(&format!("Failed to remove {} file", phc_error_bound_path));
                panic!(
                    "Unexpected failed to read PHC error bound from file: {:?}",
                    e
                );
            }
        }
        std::fs::remove_file(phc_error_bound_path)
            .expect(&format!("Failed to remove {} file", phc_error_bound_path));
    }

    #[test]
    fn test_get_phc_error_bound_from_path_nonexistent_path() {
        assert!(
            get_phc_error_bound_from_path(&std::path::PathBuf::from("SomeNonexistentPath!!"))
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        );
    }

    #[test]
    fn test_get_phc_error_bound_from_path_invalid_file_contents() {
        let phc_error_bound_path = "test_get_phc_error_bound_from_path_invalid_file_contents";
        let mut test_phc_error_bound_file = std::fs::File::create(phc_error_bound_path)
            .expect(&format!("Failed to create file {}", phc_error_bound_path));
        test_phc_error_bound_file
            .write_all("not_an_i64".as_bytes())
            .expect(&format!("Failed to write to {} file", phc_error_bound_path));
        match std::panic::catch_unwind(|| {
            get_phc_error_bound_from_path(&std::path::PathBuf::from(phc_error_bound_path))
        }) {
            Ok(_) => {
                std::fs::remove_file(phc_error_bound_path)
                    .expect(&format!("Failed to remove {} file", phc_error_bound_path));
                panic!("Expected to panic when getting get_phc_error_bound_from_path parses invalid value");
            }
            Err(e) => {
                std::fs::remove_file(phc_error_bound_path)
                    .expect(&format!("Failed to remove {} file", phc_error_bound_path));
                assert!(e
                    .downcast::<String>()
                    .unwrap()
                    .contains("Could not parse error bound value to i64"));
            }
        }
    }
}
