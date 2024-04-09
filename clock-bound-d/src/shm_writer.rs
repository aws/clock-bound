// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only

mod clock_state_fsm;

use clock_bound_shm::{ClockErrorBound, ShmWrite, ShmWriter};
use chrony_candm::reply::Tracking;
use std::path::Path;
use std::time::Duration;
use tracing::{debug, error, info};

use crate::thread_manager::Context;
use crate::{ChronyClockStatus, Message};
use clock_state_fsm::{FSMState, ShmClockState};

// TODO: make this a parameter on the CLI?
const CLOCKBOUND_SHM_DEFAULT_PATH: &str = "/var/run/clockbound/shm";

/// A struct to update the shared memory segment holding clock error bound data.
///
/// It holds a ShmWriter to write the updates to the shared memory segment. It also holds some
/// state to decide if updates should be written or withheld. The name of this structure is
/// underwhelming, could have been a bit more creative.
struct ShmUpdater<W>
where
    W: ShmWrite,
{
    /// A writer to the ClockErrorBound shared memory segment.
    writer: W,

    /// Maximum drift rate of the clock between updates of the synchronization daemon.
    max_drift_ppb: u32,

    /// FSM that tracks the status of the clock written to the SHM segment.
    shm_clock_state: Box<dyn FSMState>,

    /// The last calculated clock error bound.
    bound_nsec: i64,

    /// The time at which the clock error bound was sampled last.
    as_of: libc::timespec,

    /// Reserved field.  Place-holder that is reserved for future use.
    reserved1: u32,
}

impl<W> ShmUpdater<W>
where
    W: ShmWrite,
{
    /// Create a new ShmUpdater
    fn new(writer: W, max_drift_ppb: u32) -> ShmUpdater<W> {
        ShmUpdater {
            writer,
            shm_clock_state: Box::<ShmClockState>::default(),
            max_drift_ppb,
            bound_nsec: 0,
            as_of: libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            reserved1: 0,
        }
    }

    /// Write the clock error bound to the shared memory segment
    ///
    /// The internal state and parameters kept on the ShmUpdater allow to call this function on any
    /// external event received.
    fn write_clock_error_bound(&mut self) {
        // Set a future point in time by which a stale value of the error bound accessed by a
        // reader should not be used. For now, set it to 1000 seconds, which maps to how long a
        // linear model of drift is valid. This is fairly arbitrary and needs to be revisited.
        //
        // TODO: calibrate the value passed to void_after
        let void_after = libc::timespec {
            tv_sec: self.as_of.tv_sec + 1000,
            tv_nsec: 0,
        };

        let ceb = ClockErrorBound::new(
            self.as_of,
            void_after,
            self.bound_nsec,
            self.max_drift_ppb,
            self.reserved1,
            self.shm_clock_state.value(),
        );

        debug!("Writing ClockErrorBound to shared memory {:?}", ceb);
        self.writer.write(&ceb);
    }

    /// Process a chrony clock update message.
    ///
    /// This function computes the new parameters to the clock error bound, and execute the clock
    /// status FSM that drives the clock status stored in the shared memory segment. The clock error
    /// bound parameters are tracked and written out to the shared memory segment.
    fn process_clock_update(
        &mut self,
        tracking: Tracking,
        phc_error_bound: i64,
        as_of: libc::timespec,
    ) {
        // Quick logging that may help confirm shm_clock_state execution
        debug!(
            "Received clock update message [{:?}, {:?}]",
            tracking, as_of
        );

        // This message contains updated clock synchronization information from chrony Tracking
        // data. Extract and convert info, and keep track of this latest update.
        let (mut bound_nsec, clock_status) = extract_bound_from_tracking(tracking);
        bound_nsec += phc_error_bound;
        self.shm_clock_state = self.shm_clock_state.apply_chrony(clock_status);

        // Only update the clock error bound value if chrony is synchronized. This helps ensure
        // that the default value of root delay and root dispersion (both set to 1 second) do not
        // distort the linear growth of the clock error bound when chronyd restarts.
        if clock_status == ChronyClockStatus::Synchronized {
            self.bound_nsec = bound_nsec;
            self.as_of = as_of;
        }

        // Finally write the new CEB out to shared memory.
        self.write_clock_error_bound();
    }

    /// Process a chrony missing update message.
    ///
    /// This function executes the FSM that drives the clock status stored in the shared memory
    /// segment, applying the transition that the status of chrony is unknown, and writes out to
    /// the shared memory segment.
    fn process_missing_clock_update(&mut self, within_grace_period: bool) {
        // Quick logging that may help confirm shm_clock_state execution
        debug!("Received missing clock update message");

        // If chrony has been non-responsive for a short period of time, trust the clock is free
        // running till the end of the grace period.
        // TODO: this may be best refactored to have this logic embedded in the FSM, instead of
        // having it split between the chrony poller and the SHM Writer, once the
        // no Clock Distruption FSM is removed from the code base.
        let chrony_status = match within_grace_period {
            true => ChronyClockStatus::FreeRunning,
            false => ChronyClockStatus::Unknown,
        };

        // Execute the FSM to track clock status. This is the only parameter that changes compared
        // to previous processing of chrony updates.
        self.shm_clock_state = self.shm_clock_state.apply_chrony(chrony_status);

        // Finally write the new CEB out to shared memory.
        self.write_clock_error_bound();
    }
}

// Extract relevant information from chrony tracking data
fn extract_bound_from_tracking(tracking: Tracking) -> (i64, ChronyClockStatus) {
    let root_delay: f64 = tracking.root_delay.into();
    let root_dispersion: f64 = tracking.root_dispersion.into();
    let current_correction: f64 = tracking.current_correction.into();

    // Compute the clock error bound *at the time chrony reported the tracking data*. Remember
    // that the root dispersion reported by chrony is at the time the tracking data is
    // retrieved, not at the time of the last system clock update.
    let bound_nsec =
        ((root_delay / 2. + root_dispersion + current_correction) * 1_000_000_000.0).ceil() as i64;

    // Compute the duration since the last time chronyd updated the system clock.
    let duration_since_update = match tracking.ref_time.elapsed() {
        Ok(duration) => duration,
        Err(e) => {
            error!(
                "Failed to get duration since chronyd last clock update [{}]",
                e
            );
            return (bound_nsec, ChronyClockStatus::Unknown);
        }
    };

    // Compute the time it would take for chronyd 8-wide register to be completely empty (e.g. the
    // last 8 NTP requests timed out)
    let polling_period = f64::from(tracking.last_update_interval);
    let empty_register_timeout = Duration::from_secs((polling_period * 8.0) as u64);

    // Get the status reported by chrony and tracking data.
    // Chronyd tends to report a synchronized status for a very looooong time after it has failed
    // to continuously receive NTP responses. Here the status is over-written if the last time
    // chronyd successfully updated the system clock is "too old". Too old is define as the time it
    // would take for the 8-wide register to become empty.
    let status = match ChronyClockStatus::from(tracking.leap_status) {
        ChronyClockStatus::Synchronized => {
            if duration_since_update > empty_register_timeout {
                ChronyClockStatus::FreeRunning
            } else {
                ChronyClockStatus::Synchronized
            }
        }
        status => status,
    };

    (bound_nsec, status)
}

/// Continuously listen to messages to write to the shared memory segment.
///
/// This writer waits for instructions and eventually writes updated clock error bound to the
/// shared memory segment. This include:
/// - messages from the chrony poller to update the memory segment with fresh chrony tracking data
fn process_messages<W>(ctx: Context, mut updater: ShmUpdater<W>)
where
    W: ShmWrite,
{
    let mut keep_running = true;

    // Keep on running forever until we receive the instruction to stop.
    while keep_running {
        match ctx.mbox.recv() {
            Ok(Message::ClockErrorBoundData((tracking, phc_error_bound, as_of))) => {
                // TODO use phc_error_bound here
                updater.process_clock_update(tracking, phc_error_bound, as_of)
            }
            Ok(Message::ChronyNotRespondingGracePeriod) => {
                updater.process_missing_clock_update(true)
            }
            Ok(Message::PhcErrorBoundRetrievalFailedGracePeriod) => {
                updater.process_missing_clock_update(true)
            }
            Ok(Message::ChronyNotResponding) => updater.process_missing_clock_update(false),
            Ok(Message::PhcErrorBoundRetrievalFailed) => {
                updater.process_missing_clock_update(false)
            }
            Ok(Message::ThreadAbort) => {
                info!("Received message to stop shm writer thread");
                keep_running = false;
            }
            Ok(msg) => info!("Received message without handler {:?}", msg),
            Err(e) => error!("Error reading from MPSC channel: {:?}", e),
        }
    }
}

/// Entry point to this thread.
pub fn run(ctx: Context, max_drift_ppb: u32) {
    info!("Starting shared memory writer thread");
    // Create a writer to update the clock error bound shared memory segment
    let writer = match ShmWriter::new(Path::new(CLOCKBOUND_SHM_DEFAULT_PATH)) {
        Ok(writer) => {
            debug!("Created a new ShmWriter");
            writer
        }
        Err(e) => {
            error!(
                "Failed to create the SHM writer at {:?} {}",
                CLOCKBOUND_SHM_DEFAULT_PATH, e
            );
            panic!("Failed to create SHM writer");
        }
    };

    // Pack the writer into the updater structure.
    let updater = ShmUpdater::new(writer, max_drift_ppb);
    process_messages(ctx, updater)
}

#[cfg(test)]
mod t_shm_writer {
    use clock_bound_shm::ClockStatus;
    use chrony_candm::common::ChronyAddr;
    use chrony_candm::reply::Tracking;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::time::SystemTime;

    use super::*;
    use crate::channels::{new_channel_web, MailBox};
    use crate::{ChannelId, Message};

    // A mock writer that will store the writes instead of reaching out to the shared memory
    // segment. The type of `storage` is a bit of a mouthful, but allows for internal mutability
    // and multiple ownership which makes getting the content of storage out of the writer much
    // easier when writing the tests.
    struct MockWriter {
        pub storage: Rc<RefCell<VecDeque<ClockErrorBound>>>,
    }

    impl MockWriter {
        fn new(storage: Rc<RefCell<VecDeque<ClockErrorBound>>>) -> Self {
            Self { storage }
        }
    }

    impl ShmWrite for MockWriter {
        // Implement the ShmWrite trait, but store writes on the mock writer
        fn write(&mut self, ceb: &ClockErrorBound) {
            self.storage.borrow_mut().push_back(*ceb);
        }
    }

    // Get our MPSC channels up.
    fn setup_context() -> (Context, MailBox<ChannelId, Message>) {
        let (mut mboxes, dbox) =
            new_channel_web(vec![ChannelId::ClockErrorBoundPoller, ChannelId::ShmWriter]);
        let mbox = mboxes.get_mailbox(&ChannelId::ShmWriter).unwrap();
        (
            Context {
                mbox,
                dbox,
                channel_id: ChannelId::ShmWriter,
            },
            mboxes,
        )
    }

    // Build a "fake" Tracking struct. Put in some non-zero values to help asssert that the bound
    // on clock error is computed correctly.
    fn build_tracking() -> Tracking {
        Tracking {
            ref_id: 0,
            ip_addr: ChronyAddr::default(),
            stratum: 1,
            leap_status: 1,
            ref_time: SystemTime::now(),
            current_correction: 0.007.into(),
            last_offset: 0.0.into(),
            rms_offset: 0.0.into(),
            freq_ppm: 0.0.into(),
            resid_freq_ppm: 0.0.into(),
            skew_ppm: 0.0.into(),
            root_delay: 0.100.into(),
            root_dispersion: 0.020.into(),
            last_update_interval: 4.0.into(),
        }
    }

    // Assert that the expected value of the clock error bound is written when instructed
    #[test]
    fn test_write_correct_ceb() {
        let (ctx, _) = setup_context();
        let storage = Rc::new(RefCell::new(VecDeque::new()));
        let mock_writer = MockWriter::new(storage.clone());
        let updater = ShmUpdater::new(mock_writer, 5000);

        let tracking = build_tracking();
        let as_of = libc::timespec {
            tv_sec: 1000,
            tv_nsec: 0,
        };

        let _ = ctx.dbox.send(
            &ChannelId::ShmWriter,
            Message::ClockErrorBoundData((tracking, 0, as_of)),
        );
        let _ = ctx.dbox.send(&ChannelId::ShmWriter, Message::ThreadAbort);

        process_messages(ctx, updater);

        let expected = ClockErrorBound::new(
            as_of,
            libc::timespec {
                tv_sec: 2000,
                tv_nsec: 0,
            },
            77_000_001, // Note the value is ceil'ed
            5000,
            0,
            ClockStatus::Synchronized,
        );
        let ceb = storage.borrow_mut().pop_front().unwrap();

        assert_eq!(ceb, expected);
    }

    // Assert that the expected value of the clock error bound is written when instructed
    #[test]
    fn test_write_correct_ceb_with_phc_error_bound() {
        let (ctx, _) = setup_context();
        let storage = Rc::new(RefCell::new(VecDeque::new()));
        let mock_writer = MockWriter::new(storage.clone());
        let updater = ShmUpdater::new(mock_writer, 5000);

        let tracking = build_tracking();
        let as_of = libc::timespec {
            tv_sec: 1000,
            tv_nsec: 0,
        };
        let phc_error_bound = 12345;
        let _ = ctx.dbox.send(
            &ChannelId::ShmWriter,
            Message::ClockErrorBoundData((tracking, phc_error_bound, as_of)),
        );
        let _ = ctx.dbox.send(&ChannelId::ShmWriter, Message::ThreadAbort);

        process_messages(ctx, updater);

        let expected = ClockErrorBound::new(
            as_of,
            libc::timespec {
                tv_sec: 2000,
                tv_nsec: 0,
            },
            77_000_001 + phc_error_bound, // Note the value is ceil'ed
            5000,
            0,
            ClockStatus::Synchronized,
        );
        let ceb = storage.borrow_mut().pop_front().unwrap();

        assert_eq!(ceb, expected);
    }

    // Assert that a chrony missing data update sets the clock status back to Unknown
    #[test]
    fn test_write_ceb_when_chrony_not_responding() {
        let (ctx, mut _mboxes) = setup_context();
        let storage = Rc::new(RefCell::new(VecDeque::new()));
        let mock_writer = MockWriter::new(storage.clone());
        let updater = ShmUpdater::new(mock_writer, 5000);

        let tracking = build_tracking();
        let as_of = libc::timespec {
            tv_sec: 1000,
            tv_nsec: 0,
        };

        let _ = ctx.dbox.send(
            &ChannelId::ShmWriter,
            Message::ClockErrorBoundData((tracking, 0, as_of)),
        );
        let _ = ctx
            .dbox
            .send(&ChannelId::ShmWriter, Message::ChronyNotResponding);
        let _ = ctx.dbox.send(&ChannelId::ShmWriter, Message::ThreadAbort);

        process_messages(ctx, updater);

        let expected = ClockErrorBound::new(
            as_of,
            libc::timespec {
                tv_sec: 2000,
                tv_nsec: 0,
            },
            77_000_001, // Note the value is ceil'ed
            5000,
            0,
            ClockStatus::Synchronized,
        );
        let ceb = storage.borrow_mut().pop_front().unwrap();
        assert_eq!(ceb, expected);

        let expected = ClockErrorBound::new(
            as_of,
            libc::timespec {
                tv_sec: 2000,
                tv_nsec: 0,
            },
            77_000_001, // Note the value is ceil'ed
            5000,
            0,
            ClockStatus::Unknown,
        );
        let ceb = storage.borrow_mut().pop_front().unwrap();
        assert_eq!(ceb, expected);
    }
}
