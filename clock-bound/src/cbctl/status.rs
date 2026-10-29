//! A snapshot of the ClockBound daemon shared memory segment, for `cbctl status`.

use std::ffi::CString;

use errno::Errno;
use nix::sys::time::TimeSpec;

use crate::client_impl::VMClockSHM;
use crate::shm::{ClockErrorBound, ShmError, ShmReader, TimeWindowWithCounter};
use crate::{ClockStatus, Error, ErrorKind, TimeWindow};

/// The values of one read of the ClockBound daemon shared memory segment, and `now()` on it.
#[derive(Debug, Clone, PartialEq, bon::Builder)]
#[non_exhaustive]
pub struct Snapshot {
    /// `now()` on this snapshot, as [`Client::now`](crate::Client::now) returns it: the interval
    /// true time is within, and the clock status.
    pub now: TimeWindow,

    /// The counter value `now` was computed at.
    pub now_counter: u64,

    /// Timestamp of the last clock sync.
    pub as_of: TimeSpec,

    /// Counter value at the last clock sync, the same instant as `as_of`.
    pub as_of_counter: u64,

    /// Time after which the last clock sync is void and its error bound must not be trusted.
    pub void_after: TimeSpec,

    /// Error bound at the last clock sync, in nanoseconds, before growth.
    pub bound_nsec: i64,

    /// Maximum drift rate of the oscillator between clock syncs, in parts per billion.
    pub max_drift_ppb: u32,

    /// Counter period estimate, in seconds per tick.
    pub period: f64,

    /// Relative error of the counter period estimate, a dimensionless ratio.
    pub period_err: f64,

    /// Whether the daemon enabled clock disruption detection.
    pub disruption_support_enabled: bool,
}

/// Why [`Snapshot::read`] failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SnapshotError {
    /// The segment does not exist.
    #[error("the ClockBound shared memory segment was not found")]
    ShmNotFound {
        /// The path of the segment.
        path: String,
    },

    /// The segment exists, but the daemon has not completed writing clock parameters
    /// to it yet.
    #[error("the ClockBound shared memory segment has incomplete clock parameters")]
    ShmIncomplete {
        /// The path of the segment.
        path: String,
    },

    /// Any other failure to read the segment, including one that cannot be opened
    /// or is not the V3 layout.
    #[error("cannot read the ClockBound shared memory segment")]
    ShmUnreadable {
        /// The path of the segment.
        path: String,
        /// Why the segment cannot be read.
        cause: Error,
    },

    /// Disruption support is on, but the VMClock device cannot be read.
    #[error("disruption support is on, but the VMClock device cannot be read")]
    VMClockUnreadable {
        /// The path of the VMClock device.
        path: String,
        /// Why the device cannot be read.
        cause: Error,
    },
}

impl SnapshotError {
    /// The path the error is about: the VMClock device's for
    /// [`SnapshotError::VMClockUnreadable`], and the segment's otherwise.
    pub fn path(&self) -> &str {
        match self {
            SnapshotError::ShmNotFound { path }
            | SnapshotError::ShmIncomplete { path }
            | SnapshotError::VMClockUnreadable { path, .. }
            | SnapshotError::ShmUnreadable { path, .. } => path,
        }
    }

    /// A [`SnapshotError::ShmNotFound`] for the segment at `path`.
    fn shm_not_found(path: &str) -> SnapshotError {
        SnapshotError::ShmNotFound {
            path: path.to_owned(),
        }
    }

    /// A [`SnapshotError::ShmIncomplete`] for the segment at `path`.
    fn shm_incomplete(path: &str) -> SnapshotError {
        SnapshotError::ShmIncomplete {
            path: path.to_owned(),
        }
    }

    /// A [`SnapshotError::ShmUnreadable`] for the segment at `path`.
    fn shm_unreadable(path: &str, cause: impl Into<Error>) -> SnapshotError {
        SnapshotError::ShmUnreadable {
            path: path.to_owned(),
            cause: cause.into(),
        }
    }

    /// A [`SnapshotError::VMClockUnreadable`] for the device at `path`.
    fn vmclock_unreadable(path: &str, cause: impl Into<Error>) -> SnapshotError {
        SnapshotError::VMClockUnreadable {
            path: path.to_owned(),
            cause: cause.into(),
        }
    }
}

impl Snapshot {
    /// Read the ClockBound daemon shared memory segment at `clockbound_shm_path`, and with
    /// disruption support on, the VMClock device at `vmclock_shm_path`, to produce a status.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotError::ShmNotFound`] if the segment does not exist,
    /// [`SnapshotError::ShmIncomplete`] if its clock parameters are incomplete, and
    /// [`SnapshotError::VMClockUnreadable`] if disruption support is on and the VMClock
    /// device cannot be read. Any other failure is [`SnapshotError::ShmUnreadable`].
    pub fn read(
        clockbound_shm_path: &str,
        vmclock_shm_path: &str,
    ) -> Result<Snapshot, SnapshotError> {
        let Ok(shm_path) = CString::new(clockbound_shm_path) else {
            return Err(SnapshotError::shm_unreadable(
                clockbound_shm_path,
                Error {
                    kind: ErrorKind::Syscall,
                    errno: Errno(libc::EINVAL),
                    detail: format!(
                        "Path to clockbound daemon shared memory segment contains a NUL byte: {clockbound_shm_path:?}"
                    ),
                },
            ));
        };

        let mut reader = match ShmReader::new(shm_path.as_c_str()) {
            Ok(reader) => reader,
            Err(ShmError::SyscallError(_, errno)) if errno.0 == libc::ENOENT => {
                return Err(SnapshotError::shm_not_found(clockbound_shm_path));
            }
            // Generation 0, or a file shorter than its header while the daemon creates it.
            Err(ShmError::SegmentNotInitialized(_)) => {
                return Err(SnapshotError::shm_incomplete(clockbound_shm_path));
            }
            Err(error) => {
                return Err(SnapshotError::shm_unreadable(clockbound_shm_path, error));
            }
        };

        let ceb = match reader.snapshot() {
            Ok(ceb) => *ceb,
            // The reader gives up on a write that does not complete.
            Err(ShmError::SegmentNotInitialized(_)) => {
                return Err(SnapshotError::shm_incomplete(clockbound_shm_path));
            }
            Err(error) => {
                return Err(SnapshotError::shm_unreadable(clockbound_shm_path, error));
            }
        };

        let ClockErrorBound::V3(ceb_v3) = ceb else {
            return Err(SnapshotError::shm_unreadable(
                clockbound_shm_path,
                Error {
                    kind: ErrorKind::SegmentVersionNotSupported,
                    errno: Errno(0),
                    detail: String::from("The clockbound daemon shared memory segment is not V3"),
                },
            ));
        };

        // The reader hands back its initial, empty snapshot while the daemon is in the middle of
        // a write (odd generation), or has wiped the segment since it was opened.
        if ceb.as_of() == TimeSpec::new(0, 0) {
            return Err(SnapshotError::shm_incomplete(clockbound_shm_path));
        }

        let TimeWindowWithCounter {
            time_window,
            counter: now_counter,
        } = ceb_v3
            .now_with_counter()
            .map_err(|error| SnapshotError::shm_unreadable(clockbound_shm_path, error))?;

        // After `now()`, as in `Client::now`.
        let disrupted = VMClockSHM::new(vmclock_shm_path, ceb.clock_disruption_support_enabled())
            .and_then(|mut vmclock| {
                vmclock
                    .is_disrupted(ceb.disruption_marker())
                    .map_err(Error::from)
            })
            .map_err(|cause| SnapshotError::vmclock_unreadable(vmclock_shm_path, cause))?;
        let now = if disrupted {
            TimeWindow {
                clock_status: ClockStatus::Disrupted,
                ..time_window
            }
        } else {
            time_window
        };

        Ok(Snapshot {
            now,
            now_counter,
            as_of: ceb.as_of(),
            as_of_counter: ceb_v3.as_of_counter(),
            void_after: ceb.void_after(),
            bound_nsec: ceb.bound_nsec(),
            max_drift_ppb: ceb.max_drift_ppb(),
            period: ceb_v3.period(),
            period_err: ceb_v3.period_err(),
            disruption_support_enabled: ceb.clock_disruption_support_enabled(),
        })
    }

    /// The rate the error bound grows at, in parts per billion.
    pub fn bound_growth_ppb(&self) -> f64 {
        f64::from(self.max_drift_ppb) + self.period_err * 1e9
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::Client;
    use crate::daemon::io::tsc::read_timestamp_counter_begin;
    use crate::shm::shm_header::ShmHeader;
    use crate::shm::{ClockErrorBoundGeneric, ClockErrorBoundLayoutVersion, ShmWriter};
    use crate::vmclock::shm::VMClockShmBody;
    use crate::vmclock::shm_writer::VMClockShmWriter;

    use std::fs::OpenOptions;
    use std::mem::offset_of;
    use std::path::Path;

    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    /// The last clock sync `write_v3` writes.
    const AS_OF: TimeSpec = TimeSpec::new(1_790_703_720, 694_821_000);

    /// The disruption marker `write_v3` writes to the segment.
    const DISRUPTION_MARKER: u64 = 10;

    /// The shm1 and VMClock device paths in a temporary directory, removed when dropped.
    /// Nothing is written at either path until a test writes it.
    struct TempPaths {
        _dir: TempDir,
        shm1: String,
        vmclock: String,
    }

    #[fixture]
    fn temp_paths() -> TempPaths {
        let dir = tempfile::tempdir().expect("create temporary directory");
        let path_of = |name: &str| {
            dir.path()
                .join(name)
                .to_str()
                .expect("temporary path is UTF-8")
                .to_owned()
        };
        TempPaths {
            shm1: path_of("shm1"),
            vmclock: path_of("vmclock0"),
            _dir: dir,
        }
    }

    /// The counter period `write_v3` writes, in seconds per tick: a 3.4 GHz counter.
    const PERIOD: f64 = 1.0 / 3.4e9;

    /// Write a V3 segment at `path`: a sync at `AS_OF`, valid for 1000 s, with `PERIOD`,
    /// `DISRUPTION_MARKER`, `clock_status`, and `clock_disruption_support_enabled`. An
    /// `as_of_tsc` above any counter value makes `now()` see no elapsed time, so the interval and
    /// status are deterministic.
    fn write_v3(path: &str, clock_status: ClockStatus, clock_disruption_support_enabled: bool) {
        use crate::shm::ShmWrite;

        let mut writer = ShmWriter::new(
            Path::new(path),
            ClockErrorBoundLayoutVersion::V3,
            ClockErrorBoundLayoutVersion::V3,
        )
        .expect("create writer");
        let ceb = ClockErrorBoundGeneric::builder()
            .as_of_tsc(u64::MAX)
            .as_of(AS_OF)
            .void_after(AS_OF + TimeSpec::new(1000, 0))
            .bound_nsec(31_500)
            .period(PERIOD)
            .period_err(40e-9)
            .max_drift_ppb(15_000)
            .disruption_marker(DISRUPTION_MARKER)
            .clock_status(clock_status)
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .build(ClockErrorBoundLayoutVersion::V3);
        writer.write(&ceb);
    }

    /// Write a VMClock device segment at `path` with `disruption_marker`.
    fn write_vmclock(path: &str, disruption_marker: u64) {
        use crate::vmclock::shm_writer::VMClockShmWrite;

        let mut writer = VMClockShmWriter::new(Path::new(path)).expect("create VMClock writer");
        writer.write(&VMClockShmBody {
            disruption_marker,
            ..VMClockShmBody::default()
        });
    }

    /// A synchronized segment with disruption support on. The VMClock device is not written.
    #[fixture]
    fn synced_segment(temp_paths: TempPaths) -> TempPaths {
        write_v3(&temp_paths.shm1, ClockStatus::Synchronized, true);
        temp_paths
    }

    /// Overwrite the `u16` at byte `offset` in the header of the segment at `path`.
    fn set_header_u16(path: &str, offset: usize, value: u16) {
        use std::io::{Seek, SeekFrom, Write};

        let mut file = OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open segment");
        file.seek(SeekFrom::Start(
            u64::try_from(offset).expect("a header offset fits in a u64"),
        ))
        .expect("seek in header");
        file.write_all(&value.to_ne_bytes())
            .expect("write header field");
    }

    /// The client's view of the segment and VMClock device at `paths`.
    fn client_status(paths: &TempPaths) -> ClockStatus {
        Client::new_with_paths(&paths.shm1, &paths.vmclock)
            .expect("create the client")
            .now()
            .expect("read the client's now()")
            .clock_status
    }

    #[rstest]
    fn read_missing_segment_is_not_found(temp_paths: TempPaths) {
        let result = Snapshot::read(&temp_paths.shm1, &temp_paths.vmclock);
        assert!(
            matches!(&result, Err(SnapshotError::ShmNotFound { path }) if *path == temp_paths.shm1),
            "{result:?}"
        );
    }

    #[rstest]
    fn read_unwritten_segment_is_incomplete(temp_paths: TempPaths) {
        let _writer = ShmWriter::new(
            Path::new(&temp_paths.shm1),
            ClockErrorBoundLayoutVersion::V3,
            ClockErrorBoundLayoutVersion::V3,
        )
        .expect("create writer");
        let result = Snapshot::read(&temp_paths.shm1, &temp_paths.vmclock);
        assert!(
            matches!(result, Err(SnapshotError::ShmIncomplete { .. })),
            "{result:?}"
        );
    }

    /// Assert that a file shorter than the segment header, as while the daemon creates the
    /// segment, is reported as incomplete.
    #[rstest]
    fn read_truncated_segment_is_incomplete(temp_paths: TempPaths) {
        std::fs::write(&temp_paths.shm1, [0u8; 4]).expect("write truncated segment");
        let result = Snapshot::read(&temp_paths.shm1, &temp_paths.vmclock);
        assert!(
            matches!(result, Err(SnapshotError::ShmIncomplete { .. })),
            "{result:?}"
        );
    }

    /// Assert that a published segment left with an odd generation, as by a daemon that died in
    /// the middle of a write, is reported as incomplete.
    #[rstest]
    fn read_incomplete_write_is_incomplete(#[from(synced_segment)] paths: TempPaths) {
        set_header_u16(&paths.shm1, offset_of!(ShmHeader, generation), 3);
        let result = Snapshot::read(&paths.shm1, &paths.vmclock);
        assert!(
            matches!(result, Err(SnapshotError::ShmIncomplete { .. })),
            "{result:?}"
        );
    }

    /// Assert that a header still at version 0, as while the daemon creates the segment, is
    /// reported as a version error.
    #[rstest]
    fn read_wiped_header_is_a_version_error(temp_paths: TempPaths) {
        // The segment as the daemon has it while it creates it: a full header with version 0
        // and generation 0, before the layout version is stored.
        drop(
            ShmWriter::new(
                Path::new(&temp_paths.shm1),
                ClockErrorBoundLayoutVersion::V3,
                ClockErrorBoundLayoutVersion::V3,
            )
            .expect("create writer"),
        );
        set_header_u16(&temp_paths.shm1, offset_of!(ShmHeader, version), 0);
        let result = Snapshot::read(&temp_paths.shm1, &temp_paths.vmclock);
        assert!(
            matches!(
                result,
                Err(SnapshotError::ShmUnreadable {
                    cause: Error {
                        kind: ErrorKind::SegmentVersionNotSupported,
                        ..
                    },
                    ..
                })
            ),
            "{result:?}"
        );
    }

    /// Assert that every value comes from the segment, and `now()` from a counter value read
    /// during the call.
    #[rstest]
    fn read_returns_the_segment_values(#[from(synced_segment)] paths: TempPaths) {
        use nix::sys::time::TimeValLike;

        write_vmclock(&paths.vmclock, DISRUPTION_MARKER);

        let before = read_timestamp_counter_begin();
        let snapshot = Snapshot::read(&paths.shm1, &paths.vmclock).expect("read the segment");
        let after = read_timestamp_counter_begin();

        assert!(
            before <= snapshot.now_counter && snapshot.now_counter <= after,
            "now_counter {} outside the counter reads around the call [{before}, {after}]",
            snapshot.now_counter
        );
        assert_eq!(snapshot.now.clock_status, ClockStatus::Synchronized);
        assert_eq!(snapshot.now.earliest, AS_OF - TimeSpec::nanoseconds(31_500));
        assert_eq!(snapshot.now.latest, AS_OF + TimeSpec::nanoseconds(31_500));
        assert_eq!(snapshot.as_of, AS_OF);
        assert_eq!(snapshot.as_of_counter, u64::MAX);
        assert_eq!(snapshot.void_after, AS_OF + TimeSpec::new(1000, 0));
        assert_eq!(snapshot.bound_nsec, 31_500);
        assert_eq!(snapshot.max_drift_ppb, 15_000);
        // The segment stores the period and its error as scaled fractions, so compare with the
        // same values encoded and decoded by the V3 layout.
        let ClockErrorBound::V3(written) = ClockErrorBoundGeneric::builder()
            .period(PERIOD)
            .period_err(40e-9)
            .build(ClockErrorBoundLayoutVersion::V3)
        else {
            panic!("the builder builds the V3 layout");
        };
        assert_eq!(snapshot.period, written.period());
        assert_eq!(snapshot.period_err, written.period_err());
        assert!(
            snapshot.disruption_support_enabled,
            "disruption support should read back as enabled"
        );
    }

    /// Assert the client's disruption check: a VMClock disruption marker that differs from the
    /// segment's reads as `Disrupted`, as `Client::now` reports it.
    #[rstest]
    #[case::markers_match(DISRUPTION_MARKER, ClockStatus::Synchronized)]
    #[case::markers_differ(DISRUPTION_MARKER + 1, ClockStatus::Disrupted)]
    fn read_applies_the_client_disruption_check(
        #[from(synced_segment)] paths: TempPaths,
        #[case] vmclock_disruption_marker: u64,
        #[case] expected: ClockStatus,
    ) {
        write_vmclock(&paths.vmclock, vmclock_disruption_marker);

        let snapshot = Snapshot::read(&paths.shm1, &paths.vmclock).expect("read the segment");
        assert_eq!(
            snapshot.now.clock_status, expected,
            "VMClock disruption marker {vmclock_disruption_marker}"
        );
        assert_eq!(snapshot.as_of, AS_OF, "the segment values are kept");
        assert_eq!(
            client_status(&paths),
            expected,
            "the client should report the same"
        );
    }

    /// Assert that a published `Unknown` or `Disrupted` is kept, as `Client::now` reports it. The
    /// downgrades that depend on elapsed time are tested on `compute_bound_at_tsc` in `shm.rs`.
    #[rstest]
    #[case::published_unknown(ClockStatus::Unknown)]
    #[case::published_disrupted(ClockStatus::Disrupted)]
    fn read_status_matches_the_client(
        #[from(temp_paths)] paths: TempPaths,
        #[case] published: ClockStatus,
    ) {
        write_v3(&paths.shm1, published, true);
        write_vmclock(&paths.vmclock, DISRUPTION_MARKER);

        let snapshot = Snapshot::read(&paths.shm1, &paths.vmclock).expect("read the segment");
        assert_eq!(
            snapshot.now.clock_status, published,
            "the published status is kept"
        );
        assert_eq!(
            client_status(&paths),
            published,
            "the client should report the same"
        );
    }

    /// Assert that without disruption support the VMClock device is not read: there is none at
    /// the path.
    #[rstest]
    fn read_without_disruption_support_does_not_read_the_vmclock(temp_paths: TempPaths) {
        write_v3(&temp_paths.shm1, ClockStatus::Synchronized, false);
        let snapshot =
            Snapshot::read(&temp_paths.shm1, &temp_paths.vmclock).expect("read the segment");
        assert_eq!(
            snapshot.now.clock_status,
            ClockStatus::Synchronized,
            "the status should be the published one"
        );
    }

    /// Assert that with disruption support on, a missing VMClock device is an error, as for
    /// the client constructor.
    #[rstest]
    fn read_missing_vmclock_is_vmclock_unreadable(#[from(synced_segment)] paths: TempPaths) {
        assert!(
            Client::new_with_paths(&paths.shm1, &paths.vmclock).is_err(),
            "the client constructor should fail without the VMClock device"
        );

        let result = Snapshot::read(&paths.shm1, &paths.vmclock);
        assert!(
            matches!(
                &result,
                Err(SnapshotError::VMClockUnreadable {
                    path,
                    cause: Error {
                        kind: ErrorKind::SegmentNotInitialized,
                        ..
                    },
                }) if *path == paths.vmclock
            ),
            "{result:?}"
        );
    }

    /// Assert that with disruption support on, a VMClock path that cannot be read as a device is
    /// an error. A directory cannot be read for any user, root included.
    #[rstest]
    fn read_unreadable_vmclock_is_vmclock_unreadable(#[from(synced_segment)] paths: TempPaths) {
        std::fs::create_dir(&paths.vmclock).expect("create a directory at the VMClock path");

        let result = Snapshot::read(&paths.shm1, &paths.vmclock);
        assert!(
            matches!(result, Err(SnapshotError::VMClockUnreadable { .. })),
            "{result:?}"
        );
    }

    #[rstest]
    fn read_rejects_v2_segment(temp_paths: TempPaths) {
        use crate::shm::ShmWrite;

        let mut writer = ShmWriter::new(
            Path::new(&temp_paths.shm1),
            ClockErrorBoundLayoutVersion::V2,
            ClockErrorBoundLayoutVersion::V2,
        )
        .expect("create writer");
        writer.write(
            &ClockErrorBoundGeneric::builder()
                .as_of(TimeSpec::new(1, 0))
                .build(ClockErrorBoundLayoutVersion::V2),
        );

        let result = Snapshot::read(&temp_paths.shm1, &temp_paths.vmclock);
        assert!(
            matches!(
                result,
                Err(SnapshotError::ShmUnreadable {
                    cause: Error {
                        kind: ErrorKind::SegmentVersionNotSupported,
                        ..
                    },
                    ..
                })
            ),
            "{result:?}"
        );
    }

    /// Assert that a segment that cannot be opened is another error, with its errno. A path
    /// under a regular file fails with `ENOTDIR` for any user, root included.
    #[rstest]
    fn read_segment_that_cannot_be_opened_is_unreadable_with_errno(temp_paths: TempPaths) {
        std::fs::write(&temp_paths.shm1, b"").expect("write a regular file");
        let under_a_file = format!("{}/shm1", temp_paths.shm1);

        let result = Snapshot::read(&under_a_file, &temp_paths.vmclock);
        assert!(
            matches!(
                result,
                Err(SnapshotError::ShmUnreadable {
                    cause: Error {
                        kind: ErrorKind::Syscall,
                        errno: Errno(libc::ENOTDIR),
                        ..
                    },
                    ..
                })
            ),
            "{result:?}"
        );
    }

    #[test]
    fn bound_growth_is_the_drift_plus_the_period_error() {
        let snapshot = Snapshot {
            now: TimeWindow {
                earliest: TimeSpec::new(1, 0),
                latest: TimeSpec::new(3, 0),
                clock_status: ClockStatus::Synchronized,
            },
            now_counter: 2,
            as_of: TimeSpec::new(2, 0),
            as_of_counter: 1,
            void_after: TimeSpec::new(1002, 0),
            bound_nsec: 1,
            max_drift_ppb: 15_000,
            period: 1e-9,
            period_err: 40e-9,
            disruption_support_enabled: false,
        };
        let growth = snapshot.bound_growth_ppb();
        assert!((growth - 15_040.0).abs() < 1e-6, "growth {growth} ppb");
    }

    #[rstest]
    #[case::not_found(
        SnapshotError::shm_not_found("shm1"),
        "the ClockBound shared memory segment was not found",
        "shm1"
    )]
    #[case::incomplete(
        SnapshotError::shm_incomplete("shm1"),
        "the ClockBound shared memory segment has incomplete clock parameters",
        "shm1"
    )]
    #[case::unreadable(
        SnapshotError::shm_unreadable("shm1", ShmError::SegmentMalformed(String::new())),
        "cannot read the ClockBound shared memory segment",
        "shm1"
    )]
    #[case::vmclock_unreadable(
        SnapshotError::vmclock_unreadable("vmclock0", ShmError::SegmentMalformed(String::new())),
        "disruption support is on, but the VMClock device cannot be read",
        "vmclock0"
    )]
    fn errors_display_what_was_found_and_carry_the_path(
        #[case] error: SnapshotError,
        #[case] message: &str,
        #[case] path: &str,
    ) {
        assert_eq!(error.to_string(), message);
        assert_eq!(error.path(), path);
    }
}
