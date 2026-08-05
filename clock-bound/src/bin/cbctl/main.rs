//! `cbctl` - ClockBound control command line utility.
//!
//! A collection of commands to interact with and monitor the ClockBound daemon. Future commands
//! should follow the same format as `waitsync`: each subcommand lives in its own module, defines
//! its arguments there, returns an [`ExitCode`], and adds one variant to [`Command`].

use std::process::ExitCode;

use clap::{Parser, Subcommand};

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
    /// Polls the ClockBound daemon once per second, for up to `--timeout-seconds` seconds, until
    /// the clock status is Synchronized. A timeout of 0 waits indefinitely.
    Waitsync(waitsync::WaitsyncArgs),
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Waitsync(args) => waitsync::run(&args),
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Assert that the `--timeout-seconds` flag parses into the expected value.
    #[test]
    fn cli_parse_waitsync_timeout_seconds() {
        let cli = Cli::parse_from(["cbctl", "waitsync", "--timeout-seconds", "30"]);
        let Command::Waitsync(args) = cli.command;
        assert_eq!(args.timeout_seconds, 30);
    }

    /// Assert that the short `-t` flag parses into the expected value.
    #[test]
    fn cli_parse_waitsync_timeout_seconds_short() {
        let cli = Cli::parse_from(["cbctl", "waitsync", "-t", "30"]);
        let Command::Waitsync(args) = cli.command;
        assert_eq!(args.timeout_seconds, 30);
    }

    /// Assert that `--timeout-seconds` is optional and defaults to 60 seconds.
    #[test]
    fn cli_parse_waitsync_default_timeout_seconds() {
        let cli = Cli::parse_from(["cbctl", "waitsync"]);
        let Command::Waitsync(args) = cli.command;
        assert_eq!(args.timeout_seconds, 60);
    }

    /// Assert that a zero timeout is accepted, meaning wait indefinitely.
    #[test]
    fn cli_parse_waitsync_accepts_zero() {
        let cli = Cli::parse_from(["cbctl", "waitsync", "-t", "0"]);
        let Command::Waitsync(args) = cli.command;
        assert_eq!(args.timeout_seconds, 0);
    }

    /// Assert that negative and non-numeric timeout values are rejected at parse time.
    #[test]
    fn cli_parse_waitsync_rejects_invalid() {
        let _ = Cli::try_parse_from(["cbctl", "waitsync", "-t", "-5"]).unwrap_err();
        let _ = Cli::try_parse_from(["cbctl", "waitsync", "-t", "abc"]).unwrap_err();
    }
}
