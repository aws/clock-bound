//! `cbctl waitsync` — wait for the ClockBound daemon to synchronize the system clock.
//!
//! ```text
//! cbctl waitsync [--system-clock] [--timeout-seconds SECONDS] [--verbose]
//! ```
//!
//! The command polls once per second until the clock is ready. By default it waits indefinitely;
//! `--timeout-seconds` sets a limit in seconds, and a timeout of 0 also waits indefinitely. It
//! enables activation scripts that block until the clock is synchronized. The clock is ready when:
//!
//! - by default: the clock status read from the ClockBound daemon shared memory segment is
//!   `Synchronized`.
//! - with `--system-clock`: additionally, the system clock (`CLOCK_REALTIME`), read right after
//!   the shared memory segment, is within the tolerance window. The tolerance window is the
//!   ClockBound time window, widened equally on both sides if needed so that it is at least
//!   2 milliseconds wide in total.
//!
//! Before polling, the command validates the environment: on a virtualized (non-metal) EC2
//! instance the VMClock device is expected to exist. If it is missing, the clock error bound can
//! never be trusted on this host and the command fails immediately. Bare metal EC2 instances have
//! no VMClock device by design, and non-EC2 hosts are exempt from the check.
//!
//! Exit codes:
//! - 0: the target is ready.
//! - 1: the target did not become ready within the timeout.
//! - 2: fatal condition, waiting can never succeed. For example, this host is a virtualized EC2
//!   instance on which the VMClock device is expected but missing.

use std::path::Path;
use std::process::ExitCode;
use std::thread::sleep;
use std::time::{Duration, Instant};

use clap::Args;
use clock_bound::daemon::autodetect::Autodetect;
use clock_bound::daemon::time::Duration as CBDuration;
use clock_bound::{Client, ClockStatus, Error, SHM_DEFAULT_PATH, TimeWindow, VMCLOCK_DEFAULT_PATH};
use nix::sys::time::{TimeSpec, TimeValLike};

/// Exit code when the clock synchronized within the timeout.
const EXIT_CODE_SYNCHRONIZED: u8 = 0;

/// Exit code when the clock did not synchronize within the timeout.
const EXIT_CODE_TIMEOUT: u8 = 1;

/// Exit code when waiting can never succeed (e.g. VMClock expected but missing).
const EXIT_CODE_FATAL: u8 = 2;

/// Interval between two polls of the ClockBound shared memory segment.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Smallest tolerance in `--system-clock` mode, in nanoseconds (1 ms). The tolerance is the
/// larger of this floor and the clock error bound, so the tolerance window is at least twice
/// this wide.
///
/// Using the larger here to emphasize using the client for the "best" quality time.
const SYSTEM_CLOCK_MIN_TOLERANCE_NS: CBDuration = CBDuration::from_millis(1);

/// Wait for the ClockBound daemon to synchronize.
#[derive(Debug, Args)]
pub struct WaitsyncArgs {
    /// Also wait for the system clock (`CLOCK_REALTIME`) to be within the tolerance window: the
    /// ClockBound time window, widened equally on both sides if needed so that it is at least
    /// 2 milliseconds wide in total.
    #[arg(long)]
    pub system_clock: bool,

    /// Maximum time to wait for the clock to synchronize, in seconds. 0 waits indefinitely.
    #[arg(short = 't', long, default_value_t = 0, value_name = "SECONDS")]
    pub timeout_seconds: u64,

    /// Print the clock status of each polling attempt while waiting.
    #[arg(short, long)]
    pub verbose: bool,
}

/// The outcome of a single successful observation, with a human readable description of what
/// was observed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Observation {
    /// The target is ready, waiting is over.
    Ready(String),

    /// The target is not ready yet, keep waiting.
    NotReady(String),
}

impl Observation {
    /// The default observation: ready iff the clock status is `Synchronized`.
    fn from_clock_status(clock_status: ClockStatus) -> Self {
        let description = format!("daemon status: {clock_status:?}");
        if clock_status == ClockStatus::Synchronized {
            Observation::Ready(description)
        } else {
            Observation::NotReady(description)
        }
    }
}

/// Where the system clock reading falls relative to the tolerance window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bounds {
    /// The system clock is within the tolerance window, edges included.
    Within,

    /// The system clock is before `earliest` of the tolerance window.
    Behind {
        /// How far the system clock is behind `earliest`, in nanoseconds.
        offsides: u64,
    },

    /// The system clock is after `latest` of the tolerance window.
    Ahead {
        /// How far the system clock is ahead of `latest`, in nanoseconds.
        offsides: u64,
    },
}

/// The comparison of a system clock reading against a ClockBound time window.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SystemClockCheck {
    /// The ClockBound time window, read from the shared memory segment.
    window: TimeWindow,

    /// The system clock (`CLOCK_REALTIME`) reading, taken right after the time window.
    system_clock: TimeSpec,
}

impl SystemClockCheck {
    /// Compare the system clock reading `system_clock` against `window`.
    fn new(window: TimeWindow, system_clock: TimeSpec) -> Self {
        SystemClockCheck {
            window,
            system_clock,
        }
    }

    /// The `--system-clock` observation for this check.
    fn observation(&self) -> Observation {
        let bounds = match self.bounds() {
            Bounds::Within => String::from("within bounds"),
            Bounds::Behind { offsides } => format!("out of bounds: {offsides}ns behind"),
            Bounds::Ahead { offsides } => format!("out of bounds: {offsides}ns ahead"),
        };
        let description = format!(
            "daemon status: {:?}, tolerance: {}ns, {bounds}",
            self.window.clock_status,
            self.tolerance_ns()
        );
        if self.is_ready() {
            Observation::Ready(description)
        } else {
            Observation::NotReady(description)
        }
    }

    /// Whether the clock is synchronized and the system clock is within the tolerance window.
    fn is_ready(&self) -> bool {
        self.window.clock_status == ClockStatus::Synchronized && self.bounds() == Bounds::Within
    }

    /// Where the system clock falls relative to the tolerance window, regardless of the clock
    /// status.
    fn bounds(&self) -> Bounds {
        let tolerance_window = self.tolerance_window();
        let system_clock = self.system_clock.num_nanoseconds();
        let earliest = tolerance_window.earliest.num_nanoseconds();
        let latest = tolerance_window.latest.num_nanoseconds();
        if system_clock < earliest {
            return Bounds::Behind {
                offsides: earliest.abs_diff(system_clock),
            };
        }
        if system_clock > latest {
            return Bounds::Ahead {
                offsides: system_clock.abs_diff(latest),
            };
        }
        Bounds::Within
    }

    /// The ClockBound time window, widened on each side by `tolerance - clock error bound`, so it
    /// is at least twice [`SYSTEM_CLOCK_MIN_TOLERANCE_NS`] wide. A window already wide enough is
    /// returned unchanged.
    fn tolerance_window(&self) -> TimeWindow {
        let widen = TimeSpec::nanoseconds(self.tolerance_ns() - self.ceb_ns());
        TimeWindow {
            earliest: self.window.earliest - widen,
            latest: self.window.latest + widen,
            clock_status: self.window.clock_status,
        }
    }

    /// Half the width of the tolerance window, in nanoseconds: the larger of
    /// [`SYSTEM_CLOCK_MIN_TOLERANCE_NS`] and the clock error bound.
    ///
    /// The idea is to keep the system clock within the clock error bound
    /// reported by ClockBound as long as it is synchronized.
    ///
    /// However, have a lower limit of [`SYSTEM_CLOCK_MIN_TOLERANCE_NS`]
    /// to prevent flakiness in situations where the clock error bound
    /// is very small.
    fn tolerance_ns(&self) -> i64 {
        self.ceb_ns().max(SYSTEM_CLOCK_MIN_TOLERANCE_NS.as_nanos())
    }

    /// Clock error bound of the window, in nanoseconds: `(latest - earliest) / 2`, truncated
    /// toward zero.
    fn ceb_ns(&self) -> i64 {
        (self.window.latest.num_nanoseconds() - self.window.earliest.num_nanoseconds()) / 2
    }
}

/// Poll `observe` once per `interval` until the target is ready, up to `max_tries` attempts. A
/// `max_tries` of 0 polls indefinitely. When `verbose` is set, the outcome of each attempt is
/// printed. Returns the process exit code.
///
/// Attempts are scheduled against absolute deadlines, so the time spent in `observe` does not
/// stretch the interval between attempts.
///
/// Errors returned by `observe` carry a human readable reason and mean "could not observe at
/// this attempt" (e.g. the daemon is not running yet), which is treated as "not ready, keep
/// waiting".
fn wait_for_sync(
    max_tries: u64,
    interval: Duration,
    verbose: bool,
    mut observe: impl FnMut() -> Result<Observation, String>,
) -> ExitCode {
    let mut tries: u64 = 0;
    let mut next_attempt = Instant::now();
    loop {
        tries += 1;
        match observe() {
            Ok(Observation::Ready(description)) => {
                if verbose {
                    println!("try: {tries}, {description}");
                }
                return ExitCode::from(EXIT_CODE_SYNCHRONIZED);
            }
            Ok(Observation::NotReady(description)) => {
                if verbose {
                    println!("try: {tries}, {description}");
                }
            }
            Err(reason) => {
                if verbose {
                    println!("try: {tries}, error: {reason}");
                }
            }
        }

        // No point sleeping after the last attempt. A `max_tries` of 0 never stops trying.
        if max_tries != 0 && tries >= max_tries {
            eprintln!(
                "cbctl waitsync: clock not synchronized after {max_tries} seconds, timing out"
            );
            return ExitCode::from(EXIT_CODE_TIMEOUT);
        }

        // Sleep until the next absolute deadline rather than for a fixed duration, so the poll
        // period stays at `interval` regardless of how long `observe` took. The saturating
        // difference sleeps zero if `observe` overran the whole interval.
        next_attempt += interval;
        sleep(next_attempt.saturating_duration_since(Instant::now()));
    }
}

/// Format a [`Error`] into a human readable reason for a failed attempt.
///
/// The error kind is always included, followed by the detail message when there is one.
fn describe_error(error: &Error) -> String {
    if error.detail.is_empty() {
        format!("{:?}", error.kind)
    } else {
        format!("{:?}: {}", error.kind, error.detail)
    }
}

/// Reads clock status from the ClockBound daemon shared memory segment.
///
/// The ClockBound client is opened lazily: the daemon may not have started yet when `cbctl
/// waitsync` runs (e.g. from an instance activation script at boot), so a failure to open the
/// segment is a retryable condition rather than an error. Likewise, the client is re-opened on
/// read failures, to recover from the daemon restarting.
struct ShmObserver {
    /// Path to the ClockBound daemon shared memory segment.
    clockbound_shm_path: String,

    /// Path to the VMClock device shared memory segment.
    vmclock_shm_path: String,

    /// The ClockBound client, created on first successful open of the shared memory segment.
    client: Option<Client>,
}

impl ShmObserver {
    fn new(clockbound_shm_path: &str, vmclock_shm_path: &str) -> Self {
        ShmObserver {
            clockbound_shm_path: String::from(clockbound_shm_path),
            vmclock_shm_path: String::from(vmclock_shm_path),
            client: None,
        }
    }

    /// Observe the clock, opening (or re-opening) the shared memory segment as needed. When
    /// `system_clock` is set, the system clock is also checked against the tolerance window.
    /// Errors carry a human readable reason.
    ///
    /// In `--system-clock` mode, the system clock is read right after the shared memory segment.
    /// The small gap between the two reads biases the comparison and is accepted.
    fn observe(&mut self, system_clock: bool) -> Result<Observation, String> {
        let window = self.now()?;
        if !system_clock {
            return Ok(Observation::from_clock_status(window.clock_status));
        }
        let system_clock = nix::time::clock_gettime(nix::time::ClockId::CLOCK_REALTIME)
            .map_err(|errno| format!("clock_gettime(CLOCK_REALTIME): {errno}"))?;
        Ok(SystemClockCheck::new(window, system_clock).observation())
    }

    /// Read the current time window, opening (or re-opening) the shared memory segment as
    /// needed. Errors carry a human readable reason.
    fn now(&mut self) -> Result<TimeWindow, String> {
        if self.client.is_none() {
            match Client::new_with_paths(&self.clockbound_shm_path, &self.vmclock_shm_path) {
                Ok(client) => self.client = Some(client),
                Err(error) => return Err(describe_error(&error)),
            }
        }

        let client = self.client.as_mut().expect("client was just created above");

        match client.now() {
            Ok(window) => Ok(window),
            Err(error) => {
                // Drop the client so the next attempt re-opens the segment from scratch.
                self.client = None;
                Err(describe_error(&error))
            }
        }
    }
}

/// Run the `waitsync` command.
pub fn run(args: &WaitsyncArgs) -> ExitCode {
    // Fail fast if waiting can never succeed on this host. On a virtualized (non-metal) EC2
    // instance, the VMClock device is expected to exist; if it is missing, the instance is
    // impaired, the daemon cannot guarantee a trusted clock error bound, and waiting would
    // block forever. Bare metal EC2 instances have no VMClock device by design, and non-EC2
    // hosts (or hosts whose instance type cannot be determined from DMI) are exempt.
    let autodetect = match Autodetect::detect() {
        Ok(result) => result,
        Err(error) => {
            if args.verbose {
                eprintln!(
                    "cbctl waitsync: warning: could not determine the platform ({error}); \
                     skipping the VMClock environment check"
                );
            }
            Autodetect::Other
        }
    };
    let vmclock_required = match &autodetect {
        Autodetect::Amazon(amazon) => !amazon.is_metal(),
        Autodetect::Other => false,
    };
    if vmclock_required && !Path::new(VMCLOCK_DEFAULT_PATH).exists() {
        // TODO: add a link to public documentation on the VMClock device requirement for
        // virtualized EC2 instances to this error message.
        eprintln!(
            "cbctl waitsync: the VMClock device is expected for time synchronization in \
             virtualized EC2 environments, but was not found."
        );
        return ExitCode::from(EXIT_CODE_FATAL);
    }

    let mut observer = ShmObserver::new(SHM_DEFAULT_PATH, VMCLOCK_DEFAULT_PATH);
    wait_for_sync(args.timeout_seconds, POLL_INTERVAL, args.verbose, || {
        observer.observe(args.system_clock)
    })
}

#[cfg(test)]
mod test {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::Relaxed;

    use clock_bound::ErrorKind;
    use errno::Errno;

    use super::*;

    /// Build a default-mode observe closure replaying a fixed sequence of clock status results,
    /// repeating the last one if called more times than the sequence length. `calls` counts the
    /// invocations.
    fn sequence(
        results: Vec<Result<ClockStatus, String>>,
        calls: &AtomicUsize,
    ) -> impl FnMut() -> Result<Observation, String> + '_ {
        move || {
            let index = calls.fetch_add(1, Relaxed).min(results.len() - 1);
            results[index].clone().map(Observation::from_clock_status)
        }
    }

    /// Build a time window centered on 1000 s with the given clock error bound and status.
    fn centered_window(ceb_ns: u64, clock_status: ClockStatus) -> TimeWindow {
        let mid = TimeSpec::new(1000, 0);
        let ceb = TimeSpec::from_duration(Duration::from_nanos(ceb_ns));
        TimeWindow {
            earliest: mid - ceb,
            latest: mid + ceb,
            clock_status,
        }
    }

    /// A system clock reading offset from the window midpoint (1000 s) by `offset_ns`.
    fn system_clock(offset_ns: i64) -> TimeSpec {
        let mid = TimeSpec::new(1000, 0);
        let magnitude = TimeSpec::from_duration(Duration::from_nanos(offset_ns.unsigned_abs()));
        if offset_ns < 0 {
            mid - magnitude
        } else {
            mid + magnitude
        }
    }

    /// A check of a system clock reading offset by `offset_ns` from the midpoint of a window
    /// centered on 1000 s with the given clock error bound and status.
    fn centered_check(ceb_ns: u64, clock_status: ClockStatus, offset_ns: i64) -> SystemClockCheck {
        SystemClockCheck::new(
            centered_window(ceb_ns, clock_status),
            system_clock(offset_ns),
        )
    }

    /// Assert that a synchronized clock with the system clock inside the window is ready.
    #[test]
    fn system_clock_check_inside_window() {
        let check = centered_check(5_000_000, ClockStatus::Synchronized, 0);
        assert_eq!(check.tolerance_ns(), 5_000_000);
        assert_eq!(check.bounds(), Bounds::Within);
        assert!(check.is_ready());
    }

    /// Assert that a system clock beyond the window, on either side, is not ready, and that
    /// `offsides` is the distance past the nearest edge.
    #[test]
    fn system_clock_check_outside_window() {
        let ahead = centered_check(5_000_000, ClockStatus::Synchronized, 5_000_001);
        assert_eq!(ahead.bounds(), Bounds::Ahead { offsides: 1 });
        assert!(!ahead.is_ready());
        let behind = centered_check(5_000_000, ClockStatus::Synchronized, -5_000_007);
        assert_eq!(behind.bounds(), Bounds::Behind { offsides: 7 });
        assert!(!behind.is_ready());
    }

    /// Assert that a window narrower than 2 ms is widened equally on both sides, and that
    /// `offsides` is measured against the widened edges.
    #[test]
    fn system_clock_check_min_tolerance_widens_small_window() {
        let check = centered_check(10_000, ClockStatus::Synchronized, -900_000);
        assert_eq!(
            check.tolerance_ns(),
            SYSTEM_CLOCK_MIN_TOLERANCE_NS.as_nanos()
        );
        assert_eq!(
            check.tolerance_window(),
            TimeWindow {
                earliest: TimeSpec::new(999, 999_000_000),
                latest: TimeSpec::new(1000, 1_000_000),
                clock_status: ClockStatus::Synchronized,
            }
        );
        assert!(check.is_ready());
        assert_eq!(
            centered_check(10_000, ClockStatus::Synchronized, 1_000_001).bounds(),
            Bounds::Ahead { offsides: 1 }
        );
    }

    /// Assert that a window wider than 2 ms is used as the tolerance window unchanged.
    #[test]
    fn system_clock_check_wide_window_is_unchanged() {
        let check = centered_check(3_000_000, ClockStatus::Synchronized, 2_500_000);
        assert_eq!(check.tolerance_ns(), 3_000_000);
        assert_eq!(check.tolerance_window(), check.window);
        assert!(check.is_ready());
    }

    /// Assert that a system clock exactly at an edge is within bounds, and one nanosecond past
    /// it is not.
    #[test]
    fn system_clock_check_boundary() {
        let synchronized = ClockStatus::Synchronized;
        assert!(centered_check(2_000_000, synchronized, 2_000_000).is_ready());
        assert!(centered_check(2_000_000, synchronized, -2_000_000).is_ready());
        assert!(!centered_check(2_000_000, synchronized, 2_000_001).is_ready());
        assert!(!centered_check(2_000_000, synchronized, -2_000_001).is_ready());

        // A zero clock error bound, so the 1 ms floor sets the edges.
        assert!(centered_check(0, synchronized, 1_000_000).is_ready());
        assert!(centered_check(0, synchronized, -1_000_000).is_ready());
        assert!(!centered_check(0, synchronized, 1_000_001).is_ready());
        assert!(!centered_check(0, synchronized, -1_000_001).is_ready());
    }

    /// Assert that an odd-width window is compared against `earliest` and `latest` directly,
    /// with no truncation: both edges are within bounds.
    #[test]
    fn system_clock_check_odd_width_window() {
        // earliest = 1000 s, latest = 1000 s + 4_000_001 ns: wider than 2 ms, so not widened.
        let window = TimeWindow {
            earliest: TimeSpec::new(1000, 0),
            latest: TimeSpec::new(1000, 4_000_001),
            clock_status: ClockStatus::Synchronized,
        };
        let at_latest = SystemClockCheck::new(window.clone(), TimeSpec::new(1000, 4_000_001));
        assert_eq!(at_latest.bounds(), Bounds::Within);
        let at_earliest = SystemClockCheck::new(window.clone(), TimeSpec::new(1000, 0));
        assert_eq!(at_earliest.bounds(), Bounds::Within);
        let past_latest = SystemClockCheck::new(window.clone(), TimeSpec::new(1000, 4_000_002));
        assert_eq!(past_latest.bounds(), Bounds::Ahead { offsides: 1 });
        let before_earliest = SystemClockCheck::new(window, TimeSpec::new(999, 999_999_999));
        assert_eq!(before_earliest.bounds(), Bounds::Behind { offsides: 1 });
    }

    /// Assert that a clock that is not synchronized is not ready, even within the window.
    #[test]
    fn system_clock_check_not_synchronized_is_not_ready() {
        for clock_status in [
            ClockStatus::Unknown,
            ClockStatus::FreeRunning,
            ClockStatus::Disrupted,
        ] {
            let check = centered_check(5_000_000, clock_status, 0);
            assert_eq!(check.bounds(), Bounds::Within);
            assert!(!check.is_ready());
        }
    }

    /// Assert the system clock observation reports the daemon status, the tolerance and whether
    /// the system clock is within bounds.
    #[test]
    fn system_clock_check_observation_description() {
        assert_eq!(
            centered_check(10_000, ClockStatus::Synchronized, -1_500).observation(),
            Observation::Ready(String::from(
                "daemon status: Synchronized, tolerance: 1000000ns, within bounds"
            ))
        );
        // The tolerance window ends 1 ms after the center, so 2 ms is 1 ms past `latest`.
        assert_eq!(
            centered_check(10_000, ClockStatus::Synchronized, 2_000_000).observation(),
            Observation::NotReady(String::from(
                "daemon status: Synchronized, tolerance: 1000000ns, out of bounds: 1000000ns ahead"
            ))
        );
        assert_eq!(
            centered_check(10_000, ClockStatus::Synchronized, -3_000_000).observation(),
            Observation::NotReady(String::from(
                "daemon status: Synchronized, tolerance: 1000000ns, out of bounds: 2000000ns behind"
            ))
        );
        assert_eq!(
            centered_check(10_000, ClockStatus::FreeRunning, 0).observation(),
            Observation::NotReady(String::from(
                "daemon status: FreeRunning, tolerance: 1000000ns, within bounds"
            ))
        );
    }

    /// Assert the default observation reports the daemon status.
    #[test]
    fn clock_status_observation_description() {
        assert_eq!(
            Observation::from_clock_status(ClockStatus::Synchronized),
            Observation::Ready(String::from("daemon status: Synchronized"))
        );
        assert_eq!(
            Observation::from_clock_status(ClockStatus::Unknown),
            Observation::NotReady(String::from("daemon status: Unknown"))
        );
    }

    /// Assert that a system clock target out of window keeps waiting until it comes in window.
    #[test]
    fn wait_for_sync_system_clock_out_of_window_then_ready() {
        let observations = [
            centered_check(10_000, ClockStatus::Synchronized, 5_000_000).observation(),
            centered_check(10_000, ClockStatus::Synchronized, 0).observation(),
        ];
        let calls = AtomicUsize::new(0);
        let observe = || Ok(observations[calls.fetch_add(1, Relaxed)].clone());
        let exit_code = wait_for_sync(5, Duration::ZERO, false, observe);
        assert_eq!(exit_code, ExitCode::from(EXIT_CODE_SYNCHRONIZED));
        assert_eq!(calls.load(Relaxed), 2);
    }

    /// Assert that a describe_error message always includes the error kind, with the detail
    /// appended when present.
    #[test]
    fn describe_error_prepends_kind_to_detail() {
        let error = Error {
            kind: ErrorKind::SegmentNotInitialized,
            errno: Errno(0),
            detail: String::from("waiting for the daemon"),
        };
        assert_eq!(
            describe_error(&error),
            "SegmentNotInitialized: waiting for the daemon"
        );
    }

    /// Assert that a describe_error message with no detail is just the error kind.
    #[test]
    fn describe_error_without_detail_is_kind_only() {
        let error = Error {
            kind: ErrorKind::Syscall,
            errno: Errno(0),
            detail: String::new(),
        };
        assert_eq!(describe_error(&error), "Syscall");
    }

    /// Assert that a clock that is already synchronized succeeds on the first try.
    #[test]
    fn wait_for_sync_synchronized_immediately() {
        let calls = AtomicUsize::new(0);
        let observe = sequence(vec![Ok(ClockStatus::Synchronized)], &calls);
        let exit_code = wait_for_sync(5, Duration::ZERO, false, observe);
        assert_eq!(exit_code, ExitCode::from(EXIT_CODE_SYNCHRONIZED));
        assert_eq!(calls.load(Relaxed), 1);
    }

    /// Assert that the loop keeps polling until the clock synchronizes within the time budget.
    #[test]
    fn wait_for_sync_synchronized_after_retries() {
        let calls = AtomicUsize::new(0);
        let observe = sequence(
            vec![
                Err(String::from("failed to open shared memory segment")),
                Ok(ClockStatus::Unknown),
                Ok(ClockStatus::FreeRunning),
                Ok(ClockStatus::Synchronized),
            ],
            &calls,
        );
        let exit_code = wait_for_sync(10, Duration::ZERO, false, observe);
        assert_eq!(exit_code, ExitCode::from(EXIT_CODE_SYNCHRONIZED));
        assert_eq!(calls.load(Relaxed), 4);
    }

    /// Assert that a clock that never synchronizes times out with the timeout exit code after
    /// exactly `max_tries` attempts.
    #[test]
    fn wait_for_sync_timeout() {
        let calls = AtomicUsize::new(0);
        let observe = sequence(vec![Ok(ClockStatus::FreeRunning)], &calls);
        let exit_code = wait_for_sync(3, Duration::ZERO, false, observe);
        assert_eq!(exit_code, ExitCode::from(EXIT_CODE_TIMEOUT));
        assert_eq!(calls.load(Relaxed), 3);
    }

    /// Assert that a disrupted clock is treated as not synchronized and leads to a timeout.
    #[test]
    fn wait_for_sync_disrupted_times_out() {
        let calls = AtomicUsize::new(0);
        let observe = sequence(vec![Ok(ClockStatus::Disrupted)], &calls);
        let exit_code = wait_for_sync(2, Duration::ZERO, false, observe);
        assert_eq!(exit_code, ExitCode::from(EXIT_CODE_TIMEOUT));
        assert_eq!(calls.load(Relaxed), 2);
    }

    /// Assert that a `max_tries` of 0 waits indefinitely, polling past any fixed limit until
    /// the clock synchronizes.
    #[test]
    fn wait_for_sync_zero_waits_indefinitely() {
        let mut results = vec![Ok(ClockStatus::FreeRunning); 100];
        results.push(Ok(ClockStatus::Synchronized));
        let calls = AtomicUsize::new(0);
        let observe = sequence(results, &calls);
        let exit_code = wait_for_sync(0, Duration::ZERO, false, observe);
        assert_eq!(exit_code, ExitCode::from(EXIT_CODE_SYNCHRONIZED));
        assert_eq!(calls.load(Relaxed), 101);
    }

    /// Assert that persistent observation errors (e.g. daemon never starts) lead to a timeout.
    #[test]
    fn wait_for_sync_persistent_error_times_out() {
        let calls = AtomicUsize::new(0);
        let observe = sequence(vec![Err(String::from("daemon not running"))], &calls);
        let exit_code = wait_for_sync(4, Duration::ZERO, false, observe);
        assert_eq!(exit_code, ExitCode::from(EXIT_CODE_TIMEOUT));
        assert_eq!(calls.load(Relaxed), 4);
    }
}
