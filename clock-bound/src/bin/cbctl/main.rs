//! `cbctl` - ClockBound control command line utility.
//!
//! A collection of commands to interact with and monitor the ClockBound daemon. Future commands
//! should follow the same format as `waitsync`: each subcommand lives in its own module, defines
//! its arguments there, returns an [`ExitCode`], and adds one variant to [`Command`].

use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod render;
mod sources;
mod waitsync;

/// ClockBound control command line utility.
#[derive(Debug, Parser)]
#[command(name = "cbctl", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Wait for the ClockBound daemon to synchronize the system clock.
    ///
    /// Polls once per second, for up to `--timeout-seconds` seconds, until the clock status in the
    /// ClockBound daemon shared memory segment is Synchronized. A timeout of 0 waits indefinitely.
    ///
    /// With `--system-clock`, also waits for the system clock (`CLOCK_REALTIME`) to be within the
    /// tolerance window: the ClockBound time window, widened equally on both sides to be at least
    /// 2 ms wide.
    Waitsync(waitsync::WaitsyncArgs),

    /// Display the state of the time sources configured with the ClockBound daemon.
    ///
    /// Reads the most recent snapshot from the ClockBound daemon `sources.log`. Requires sources
    /// logging to be enabled in the daemon configuration.
    ///
    /// Ages in the SELECTED column are rounded down to the second under a minute, and to the
    /// minute (or coarser) beyond, so sources deselected close together can show the same age.
    Sources(sources::SourcesArgs),
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Waitsync(args) => waitsync::run(&args),
        Command::Sources(args) => sources::run(&args),
    }
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use super::*;

    /// Assert that the `--timeout-seconds` flag parses into the expected value.
    #[test]
    fn cli_parse_waitsync_timeout_seconds() {
        let cli = Cli::parse_from(["cbctl", "waitsync", "--timeout-seconds", "30"]);
        let Command::Waitsync(args) = cli.command else {
            panic!("expected the waitsync command");
        };
        assert_eq!(args.timeout_seconds, 30);
    }

    /// Assert that the short `-t` flag parses into the expected value.
    #[test]
    fn cli_parse_waitsync_timeout_seconds_short() {
        let cli = Cli::parse_from(["cbctl", "waitsync", "-t", "30"]);
        let Command::Waitsync(args) = cli.command else {
            panic!("expected the waitsync command");
        };
        assert_eq!(args.timeout_seconds, 30);
    }

    /// Assert that `--timeout-seconds` is optional and defaults to 60 seconds.
    #[test]
    fn cli_parse_waitsync_default_timeout_seconds() {
        let cli = Cli::parse_from(["cbctl", "waitsync"]);
        let Command::Waitsync(args) = cli.command else {
            panic!("expected the waitsync command");
        };
        assert_eq!(args.timeout_seconds, 60);
    }

    /// Assert that a zero timeout is accepted, meaning wait indefinitely.
    #[test]
    fn cli_parse_waitsync_accepts_zero() {
        let cli = Cli::parse_from(["cbctl", "waitsync", "-t", "0"]);
        let Command::Waitsync(args) = cli.command else {
            panic!("expected the waitsync command");
        };
        assert_eq!(args.timeout_seconds, 0);
    }

    /// Assert that `--system-clock` is optional and off by default.
    #[test]
    fn cli_parse_waitsync_default_system_clock_is_off() {
        let cli = Cli::parse_from(["cbctl", "waitsync"]);
        let Command::Waitsync(args) = cli.command else {
            panic!("expected the waitsync command");
        };
        assert!(!args.system_clock);
    }

    /// Assert that the `--system-clock` flag parses.
    #[test]
    fn cli_parse_waitsync_system_clock() {
        let cli = Cli::parse_from(["cbctl", "waitsync", "--system-clock"]);
        let Command::Waitsync(args) = cli.command else {
            panic!("expected the waitsync command");
        };
        assert!(args.system_clock);
        assert_eq!(args.timeout_seconds, 60);
    }

    /// Assert that `--system-clock` combines with other flags in any order.
    #[test]
    fn cli_parse_waitsync_system_clock_with_other_flags() {
        let cli = Cli::parse_from(["cbctl", "waitsync", "-t", "30", "--system-clock", "-v"]);
        let Command::Waitsync(args) = cli.command else {
            panic!("expected the waitsync command");
        };
        assert!(args.system_clock);
        assert_eq!(args.timeout_seconds, 30);
        assert!(args.verbose);
    }

    /// Assert that negative and non-numeric timeout values are rejected at parse time.
    #[test]
    fn cli_parse_waitsync_rejects_invalid() {
        let _ = Cli::try_parse_from(["cbctl", "waitsync", "-t", "-5"]).unwrap_err();
        let _ = Cli::try_parse_from(["cbctl", "waitsync", "-t", "abc"]).unwrap_err();
    }

    /// Assert that `--log-dir` parses into the expected path.
    #[test]
    fn cli_parse_sources_log_dir() {
        let cli = Cli::parse_from(["cbctl", "sources", "--log-dir", "/custom/dir"]);
        let Command::Sources(args) = cli.command else {
            panic!("expected the sources command");
        };
        assert_eq!(args.log_dir, Some(PathBuf::from("/custom/dir")));
    }

    /// Assert that `--log-dir` is optional.
    #[test]
    fn cli_parse_sources_default_log_dir() {
        let cli = Cli::parse_from(["cbctl", "sources"]);
        let Command::Sources(args) = cli.command else {
            panic!("expected the sources command");
        };
        assert_eq!(args.log_dir, None);
    }
}
