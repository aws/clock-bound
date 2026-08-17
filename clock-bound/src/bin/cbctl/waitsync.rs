//! `cbctl waitsync` — wait for the ClockBound daemon to synchronize the system clock.
//!
//! ```text
//! cbctl waitsync [--timeout-seconds SECONDS]
//! ```
//!
//! The command polls the ClockBound daemon shared memory segment once per second, for up to
//! `--timeout-seconds` seconds (60 by default), until the clock status is `Synchronized`. A
//! timeout of 0 waits indefinitely. It enables activation scripts that block until the clock is
//! synchronized.
//!
//! Before polling, the command validates the environment: on a virtualized (non-metal) EC2
//! instance the VMClock device is expected to exist. If it is missing, the clock error bound can
//! never be trusted on this host and the command fails immediately. Bare metal EC2 instances have
//! no VMClock device by design, and non-EC2 hosts are exempt from the check.
//!
//! Exit codes:
//! - 0: the clock is synchronized.
//! - 1: the clock did not synchronize within the timeout.
//! - 2: fatal condition, waiting can never succeed. For example, this host is a virtualized EC2
//!   instance on which the VMClock device is expected but missing.

use std::path::Path;
use std::process::ExitCode;
use std::thread::sleep;
use std::time::{Duration, Instant};

use clap::Args;
use clock_bound::client::{
    CLOCKBOUND_SHM_CLIENT_DEFAULT_PATH, ClockBoundClient, ClockBoundError, ClockStatus,
    VMCLOCK_SHM_DEFAULT_PATH,
};
use clock_bound::daemon::autodetect::Autodetect;

/// Exit code when the clock synchronized within the timeout.
const EXIT_CODE_SYNCHRONIZED: u8 = 0;

/// Exit code when the clock did not synchronize within the timeout.
const EXIT_CODE_TIMEOUT: u8 = 1;

/// Exit code when waiting can never succeed (e.g. VMClock expected but missing).
const EXIT_CODE_FATAL: u8 = 2;

/// Interval between two polls of the ClockBound shared memory segment.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Wait for the ClockBound daemon to synchronize the system clock.
#[derive(Debug, Args)]
pub struct WaitsyncArgs {
    /// Maximum time to wait for the clock to synchronize, in seconds. 0 waits indefinitely.
    #[arg(short = 't', long, default_value_t = 60, value_name = "SECONDS")]
    pub timeout_seconds: u64,

    /// Print the clock status of each polling attempt while waiting.
    #[arg(short, long)]
    pub verbose: bool,
}

/// Poll `observe` once per `interval` until the clock is synchronized, up to `max_tries`
/// attempts. A `max_tries` of 0 polls indefinitely. When `verbose` is set, the outcome of each
/// attempt is printed. Returns the process exit code.
///
/// Attempts are scheduled against absolute deadlines, so the time spent in `observe` does not
/// stretch the interval between attempts.
///
/// Errors returned by `observe` carry a human readable reason and mean "could not observe at
/// this attempt" (e.g. the daemon is not running yet), which is treated as "not synchronized,
/// keep waiting".
fn wait_for_sync(
    max_tries: u64,
    interval: Duration,
    verbose: bool,
    mut observe: impl FnMut() -> Result<ClockStatus, String>,
) -> ExitCode {
    let mut tries: u64 = 0;
    let mut next_attempt = Instant::now();
    loop {
        tries += 1;
        match observe() {
            Ok(ClockStatus::Synchronized) => {
                if verbose {
                    println!("try: {tries}, status: Synchronized");
                }
                return ExitCode::from(EXIT_CODE_SYNCHRONIZED);
            }
            Ok(status) => {
                if verbose {
                    println!("try: {tries}, status: {status:?}");
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

/// Format a [`ClockBoundError`] into a human readable reason for a failed attempt.
///
/// The error kind is always included, followed by the detail message when there is one.
fn describe_error(error: &ClockBoundError) -> String {
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
    client: Option<ClockBoundClient>,
}

impl ShmObserver {
    fn new(clockbound_shm_path: &str, vmclock_shm_path: &str) -> Self {
        ShmObserver {
            clockbound_shm_path: String::from(clockbound_shm_path),
            vmclock_shm_path: String::from(vmclock_shm_path),
            client: None,
        }
    }

    /// Read the current clock status, opening (or re-opening) the shared memory segment as
    /// needed. Errors carry a human readable reason.
    fn observe(&mut self) -> Result<ClockStatus, String> {
        if self.client.is_none() {
            match ClockBoundClient::new_with_paths(
                &self.clockbound_shm_path,
                &self.vmclock_shm_path,
            ) {
                Ok(client) => self.client = Some(client),
                Err(error) => return Err(describe_error(&error)),
            }
        }

        let client = self.client.as_mut().expect("client was just created above");

        match client.now() {
            Ok(result) => Ok(result.clock_status),
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
    if vmclock_required && !Path::new(VMCLOCK_SHM_DEFAULT_PATH).exists() {
        // TODO: add a link to public documentation on the VMClock device requirement for
        // virtualized EC2 instances to this error message.
        eprintln!(
            "cbctl waitsync: the VMClock device is expected for time synchronization in \
             virtualized EC2 environments, but was not found."
        );
        return ExitCode::from(EXIT_CODE_FATAL);
    }

    let mut observer =
        ShmObserver::new(CLOCKBOUND_SHM_CLIENT_DEFAULT_PATH, VMCLOCK_SHM_DEFAULT_PATH);
    wait_for_sync(args.timeout_seconds, POLL_INTERVAL, args.verbose, || {
        observer.observe()
    })
}

#[cfg(test)]
mod test {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::Relaxed;

    use clock_bound::client::ClockBoundErrorKind;
    use errno::Errno;

    use super::*;

    /// Build an observe closure replaying a fixed sequence of results, repeating the last one
    /// if called more times than the sequence length. `calls` counts the invocations.
    fn sequence(
        results: Vec<Result<ClockStatus, String>>,
        calls: &AtomicUsize,
    ) -> impl FnMut() -> Result<ClockStatus, String> + '_ {
        move || {
            let index = calls.fetch_add(1, Relaxed).min(results.len() - 1);
            results[index].clone()
        }
    }

    /// Assert that a describe_error message always includes the error kind, with the detail
    /// appended when present.
    #[test]
    fn describe_error_prepends_kind_to_detail() {
        let error = ClockBoundError {
            kind: ClockBoundErrorKind::SegmentNotInitialized,
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
        let error = ClockBoundError {
            kind: ClockBoundErrorKind::Syscall,
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
