use std::fmt;
use std::process::ExitCode;

use clock_bound::cbctl::status;
use clock_bound::daemon::logging::ClockStatusName;
use clock_bound::daemon::time::tsc::{Frequency, Period, Skew};
use clock_bound::daemon::time::{Duration, Instant};
use clock_bound::{ClockStatus, Error, SHM_DEFAULT_PATH, VMCLOCK_DEFAULT_PATH};

use crate::render;

const EXIT_CODE_SUCCESS: u8 = 0;

const EXIT_CODE_FAILURE: u8 = 1;

const VALUES_COLUMN_START: usize = 30;

/// Run the `status` command.
pub fn run() -> ExitCode {
    print_outcome(status::Snapshot::read(
        SHM_DEFAULT_PATH,
        VMCLOCK_DEFAULT_PATH,
    ))
}

/// Print the outcome of a read, and return its exit code.
fn print_outcome(outcome: Result<status::Snapshot, status::SnapshotError>) -> ExitCode {
    match outcome {
        Ok(snapshot) => {
            print!("{}", StatusOutput::from(&snapshot));
            ExitCode::from(EXIT_CODE_SUCCESS)
        }
        Err(error) => {
            eprint!("{}", describe_error(&error));
            ExitCode::from(EXIT_CODE_FAILURE)
        }
    }
}

/// The values `cbctl status` prints, rendered.
struct StatusOutput {
    clock_status: &'static str,
    current_time: String,
    current_time_error_bound: String,
    current_time_error_bound_growth: String,
    current_time_counter_value: u64,
    last_clock_sync: String,
    last_clock_sync_error_bound: String,
    last_clock_sync_counter_value: u64,
    last_clock_sync_counter_period: String,
    last_clock_sync_counter_period_error: String,
    last_clock_sync_counter_frequency: String,
    last_clock_sync_valid_until: String,
    disruption_support: &'static str,
}

impl From<&status::Snapshot> for StatusOutput {
    fn from(snapshot: &status::Snapshot) -> Self {
        let (earliest, latest) = (
            Instant::from_timespec(snapshot.now.earliest),
            Instant::from_timespec(snapshot.now.latest),
        );
        let now = earliest.midpoint(latest);
        let (as_of, void_after) = (
            Instant::from_timespec(snapshot.as_of),
            Instant::from_timespec(snapshot.void_after),
        );
        let period = Some(snapshot.period)
            .filter(|period| *period > 0.0)
            .expect("the daemon writes a positive counter period");

        let (current_time, current_time_error_bound, current_time_error_bound_growth) = if matches!(
            snapshot.now.clock_status,
            ClockStatus::Synchronized | ClockStatus::FreeRunning
        ) {
            (
                render::timestamp(now),
                render::error_bound((latest - earliest) / 2),
                render::rate(Skew::from_ppb(snapshot.bound_growth_ppb())),
            )
        } else {
            (String::from("-"), String::from("-"), String::from("-"))
        };

        let (last_clock_sync, last_clock_sync_valid_until) =
            if snapshot.now.clock_status == ClockStatus::Disrupted {
                (render::timestamp(as_of), render::timestamp(void_after))
            } else {
                let (synced, void) = (render::timestamp(as_of), render::timestamp(void_after));
                let (age, left) = (
                    render::ago(now - as_of),
                    render::left_or_ago(void_after - now),
                );
                (format!("{synced}  ({age})"), format!("{void}  ({left})"))
            };

        Self {
            clock_status: ClockStatusName::from(snapshot.now.clock_status).as_str(),
            current_time,
            current_time_error_bound,
            current_time_error_bound_growth,
            current_time_counter_value: snapshot.now_counter,
            last_clock_sync,
            last_clock_sync_error_bound: render::error_bound(Duration::from_nanos(
                snapshot.bound_nsec,
            )),
            last_clock_sync_counter_value: snapshot.as_of_counter,
            last_clock_sync_counter_period: render::period(Period::from_seconds(period)),
            last_clock_sync_counter_period_error: render::rate(Skew::from_ppb(
                snapshot.period_err * 1e9,
            )),
            last_clock_sync_counter_frequency: render::frequency(Frequency::from_hz(1.0 / period)),
            last_clock_sync_valid_until,
            disruption_support: if snapshot.disruption_support_enabled {
                "On"
            } else {
                "Off"
            },
        }
    }
}

impl fmt::Display for StatusOutput {
    /// Lay the values out as labeled lines in three sections
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sections: [&[(&str, &dyn fmt::Display)]; 3] = [
            &[
                ("Clock status", &self.clock_status),
                ("Current time", &self.current_time),
                ("  Clock error bound (+/-)", &self.current_time_error_bound),
                (
                    "  Clock error bound growth",
                    &self.current_time_error_bound_growth,
                ),
                ("  Counter value", &self.current_time_counter_value),
            ],
            &[
                ("Last clock sync", &self.last_clock_sync),
                (
                    "  Clock error bound (+/-)",
                    &self.last_clock_sync_error_bound,
                ),
                ("  Counter value", &self.last_clock_sync_counter_value),
                ("  Counter period", &self.last_clock_sync_counter_period),
                (
                    "  Counter period error (+/-)",
                    &self.last_clock_sync_counter_period_error,
                ),
                (
                    "  Counter frequency",
                    &self.last_clock_sync_counter_frequency,
                ),
                ("  Valid until", &self.last_clock_sync_valid_until),
            ],
            &[("Disruption support", &self.disruption_support)],
        ];
        for (index, section) in sections.iter().enumerate() {
            if index > 0 {
                writeln!(f)?;
            }
            for (label, value) in *section {
                writeln!(f, "{label:<VALUES_COLUMN_START$}{value}")?;
            }
        }
        Ok(())
    }
}

/// Format the error for a failed read: what was found, its path, and any cause.
fn describe_error(error: &status::SnapshotError) -> String {
    let cause = match error {
        status::SnapshotError::VMClockUnreadable { cause, .. }
        | status::SnapshotError::ShmUnreadable { cause, .. } => describe_cause(cause),
        _ => String::new(),
    };
    format!("cbctl status: {error}\n({}).\n{cause}", error.path())
}

/// Format the cause of a failure: its kind, detail, and errno.
fn describe_cause(cause: &Error) -> String {
    let detail = if cause.detail.is_empty() {
        String::new()
    } else {
        format!(": {}", cause.detail)
    };
    let errno = if cause.errno.0 == 0 {
        String::new()
    } else {
        format!(" ({})", cause.errno)
    };
    format!("{:?}{detail}{errno}\n", cause.kind)
}

#[cfg(test)]
mod test {
    use clock_bound::{ErrorKind, TimeWindow};
    use errno::Errno;
    use indoc::indoc;
    use nix::sys::time::{TimeSpec, TimeValLike};
    use rstest::{fixture, rstest};

    use super::*;

    const CLOCKBOUND_SHM_PATH: &str = "/var/run/clockbound/shm1";

    const VMCLOCK_SHM_PATH: &str = "/dev/vmclock0";

    fn as_of() -> TimeSpec {
        TimeSpec::new(1_790_703_720, 694_821_000)
    }

    const AS_OF_COUNTER: u64 = 3_847_592_014_882;

    /// An example snapshot, `elapsed_nanos` after the last clock sync: a 31.5 us bound, 15 ppm
    /// drift, a 3.4 GHz counter with a 40 ppb period error, valid for 1000 s.
    fn snapshot_after(
        elapsed_nanos: i64,
        clock_status: ClockStatus,
        bound_now_nanos: i64,
    ) -> status::Snapshot {
        let now = as_of() + TimeSpec::nanoseconds(elapsed_nanos);
        let bound_now = TimeSpec::nanoseconds(bound_now_nanos);
        let elapsed_ticks = u64::try_from(elapsed_nanos * 34 / 10).expect("a forward elapsed time");
        status::Snapshot::builder()
            .now(TimeWindow {
                earliest: now - bound_now,
                latest: now + bound_now,
                clock_status,
            })
            .now_counter(AS_OF_COUNTER + elapsed_ticks)
            .as_of(as_of())
            .as_of_counter(AS_OF_COUNTER)
            .void_after(as_of() + TimeSpec::new(1000, 0))
            .bound_nsec(31_500)
            .max_drift_ppb(15_000)
            .period(1.0 / 3.4e9)
            .period_err(40e-9)
            .disruption_support_enabled(true)
            .build()
    }

    /// The synchronized example, 2.4 s after the sync: `31500 + 2.4 x 15040 = 67596 ns`.
    #[fixture]
    fn synchronized() -> status::Snapshot {
        snapshot_after(2_400_000_000, ClockStatus::Synchronized, 67_596)
    }

    #[rstest]
    fn render_synchronized(synchronized: status::Snapshot) {
        assert_eq!(
            StatusOutput::from(&synchronized).to_string(),
            indoc! {"
                Clock status                  SYNCHRONIZED
                Current time                  2026-09-29T17:42:03.094821000Z
                  Clock error bound (+/-)     67.60 us
                  Clock error bound growth    15040 ppb
                  Counter value               3855752014882

                Last clock sync               2026-09-29T17:42:00.694821000Z  (2s ago)
                  Clock error bound (+/-)     31.50 us
                  Counter value               3847592014882
                  Counter period              0.29411765 ns
                  Counter period error (+/-)  40 ppb
                  Counter frequency           3.4000000 GHz
                  Valid until                 2026-09-29T17:58:40.694821000Z  (16m 38s left)

                Disruption support            On
            "}
        );
    }

    /// Assert the free running example, 784 s after the sync: `31.5 us + 784 s x 15.04 us/s`, which
    /// is 11.82286 ms, rounded up to 11.83 ms.
    #[test]
    fn render_free_running() {
        let snapshot = snapshot_after(784_000_000_000, ClockStatus::FreeRunning, 11_822_860);
        assert_eq!(
            StatusOutput::from(&snapshot).to_string(),
            indoc! {"
                Clock status                  FREE_RUNNING
                Current time                  2026-09-29T17:55:04.694821000Z
                  Clock error bound (+/-)     11.83 ms
                  Clock error bound growth    15040 ppb
                  Counter value               6513192014882

                Last clock sync               2026-09-29T17:42:00.694821000Z  (13m 4s ago)
                  Clock error bound (+/-)     31.50 us
                  Counter value               3847592014882
                  Counter period              0.29411765 ns
                  Counter period error (+/-)  40 ppb
                  Counter frequency           3.4000000 GHz
                  Valid until                 2026-09-29T17:58:40.694821000Z  (3m 36s left)

                Disruption support            On
            "}
        );
    }

    /// Assert the disrupted example, 40 s after the sync.
    #[test]
    fn render_disrupted() {
        let snapshot = snapshot_after(40_000_000_000, ClockStatus::Disrupted, 633_100);
        assert_eq!(
            StatusOutput::from(&snapshot).to_string(),
            indoc! {"
                Clock status                  DISRUPTED
                Current time                  -
                  Clock error bound (+/-)     -
                  Clock error bound growth    -
                  Counter value               3983592014882

                Last clock sync               2026-09-29T17:42:00.694821000Z
                  Clock error bound (+/-)     31.50 us
                  Counter value               3847592014882
                  Counter period              0.29411765 ns
                  Counter period error (+/-)  40 ppb
                  Counter frequency           3.4000000 GHz
                  Valid until                 2026-09-29T17:58:40.694821000Z

                Disruption support            On
            "}
        );
    }

    /// Assert the expired example, 1022 s after the sync, past `void_after`.
    #[test]
    fn render_unknown_expired() {
        let snapshot = snapshot_after(1_022_000_000_000, ClockStatus::Unknown, 15_402_380);
        assert_eq!(
            StatusOutput::from(&snapshot).to_string(),
            indoc! {"
                Clock status                  UNKNOWN
                Current time                  -
                  Clock error bound (+/-)     -
                  Clock error bound growth    -
                  Counter value               7322392014882

                Last clock sync               2026-09-29T17:42:00.694821000Z  (17m 2s ago)
                  Clock error bound (+/-)     31.50 us
                  Counter value               3847592014882
                  Counter period              0.29411765 ns
                  Counter period error (+/-)  40 ppb
                  Counter frequency           3.4000000 GHz
                  Valid until                 2026-09-29T17:58:40.694821000Z  (22s ago)

                Disruption support            On
            "}
        );
    }

    #[rstest]
    fn render_disruption_support_off(mut synchronized: status::Snapshot) {
        synchronized.disruption_support_enabled = false;
        assert_eq!(StatusOutput::from(&synchronized).disruption_support, "Off");
    }

    /// Assert that the output is ASCII only under every clock status.
    #[rstest]
    fn render_status_is_ascii(
        #[values(
            ClockStatus::Synchronized,
            ClockStatus::FreeRunning,
            ClockStatus::Disrupted,
            ClockStatus::Unknown
        )]
        status: ClockStatus,
    ) {
        let rendered =
            StatusOutput::from(&snapshot_after(2_400_000_000, status, 67_596)).to_string();
        assert!(rendered.is_ascii(), "{status:?}:\n{rendered}");
    }

    #[rstest]
    #[case::not_found(
        status::SnapshotError::ShmNotFound {
            path: CLOCKBOUND_SHM_PATH.to_owned(),
        },
        "cbctl status: the ClockBound shared memory segment was not found\n\
         (/var/run/clockbound/shm1).\n"
    )]
    #[case::incomplete(
        status::SnapshotError::ShmIncomplete {
            path: CLOCKBOUND_SHM_PATH.to_owned(),
        },
        "cbctl status: the ClockBound shared memory segment has incomplete clock parameters\n\
         (/var/run/clockbound/shm1).\n"
    )]
    #[case::vmclock_unreadable(
        status::SnapshotError::VMClockUnreadable {
            path: VMCLOCK_SHM_PATH.to_owned(),
            cause: Error {
                kind: ErrorKind::SegmentNotInitialized,
                errno: Errno(0),
                detail: String::from(
                    "Path to VMClock device shared memory segment does not exist: /dev/vmclock0",
                ),
            },
        },
        "cbctl status: disruption support is on, but the VMClock device cannot be read\n\
         (/dev/vmclock0).\nSegmentNotInitialized: Path to VMClock device shared memory \
         segment does not exist: /dev/vmclock0\n"
    )]
    #[case::malformed(
        status::SnapshotError::ShmUnreadable {
            path: CLOCKBOUND_SHM_PATH.to_owned(),
            cause: Error {
                kind: ErrorKind::SegmentMalformed,
                errno: Errno(0),
                detail: String::from("bad magic"),
            },
        },
        "cbctl status: cannot read the ClockBound shared memory segment\n\
         (/var/run/clockbound/shm1).\nSegmentMalformed: bad magic\n"
    )]
    #[case::version_not_supported(
        status::SnapshotError::ShmUnreadable {
            path: CLOCKBOUND_SHM_PATH.to_owned(),
            cause: Error {
                kind: ErrorKind::SegmentVersionNotSupported,
                errno: Errno(0),
                detail: String::new(),
            },
        },
        "cbctl status: cannot read the ClockBound shared memory segment\n\
         (/var/run/clockbound/shm1).\nSegmentVersionNotSupported\n"
    )]
    #[case::syscall_with_errno(
        status::SnapshotError::ShmUnreadable {
            path: CLOCKBOUND_SHM_PATH.to_owned(),
            cause: Error {
                kind: ErrorKind::Syscall,
                errno: Errno(libc::EACCES),
                detail: String::from("open failed"),
            },
        },
        &format!(
            "cbctl status: cannot read the ClockBound shared memory segment\n\
             (/var/run/clockbound/shm1).\nSyscall: open failed ({})\n",
            Errno(libc::EACCES)
        )
    )]
    fn describe_error_names_what_was_found_and_its_cause(
        #[case] error: status::SnapshotError,
        #[case] expected: &str,
    ) {
        assert_eq!(describe_error(&error), expected);
    }

    /// Assert that the clock state exits 0 whatever the status, and a failed read exits 1.
    #[rstest]
    fn print_outcome_exit_codes(
        #[values(
            ClockStatus::Synchronized,
            ClockStatus::FreeRunning,
            ClockStatus::Disrupted,
            ClockStatus::Unknown
        )]
        status: ClockStatus,
    ) {
        let snapshot = snapshot_after(2_400_000_000, status, 67_596);
        assert_eq!(
            print_outcome(Ok(snapshot)),
            ExitCode::from(EXIT_CODE_SUCCESS)
        );
        let error = status::SnapshotError::ShmNotFound {
            path: CLOCKBOUND_SHM_PATH.to_owned(),
        };
        assert_eq!(print_outcome(Err(error)), ExitCode::from(EXIT_CODE_FAILURE));
    }
}
