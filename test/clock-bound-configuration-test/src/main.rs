//! Integration tests for the daemon's configuration handling.
//!
//! These flex daemon's command-line surface, how a rejected configuration reaches
//! the user, the log level, and which stream files a configuration produces.
//!
//! Every case runs in order and asserts as it goes, so the first failure stops the
//! run and names itself. None of them needs privileges, since the configuration is
//! read before any hardware is touched.

use clap::Parser;
use clock_bound::daemon::logging::StructuredLog;
use indoc::indoc;
use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a daemon started by these tests is given before it is stopped.
const DEADLINE: Duration = Duration::from_secs(10);

/// Every configurable log stream: the `[logging.<key>]` table that turns it on and
/// the file it writes. Add a stream here so the cases below cover it.
const STREAMS: &[(&str, &str)] = &[
    ("ffevents", StructuredLog::FFEvents.file_name()),
    (
        "synchronization",
        StructuredLog::Synchronization.file_name(),
    ),
];

/// Command-line arguments for the configuration test.
#[derive(Debug, Parser)]
#[command(
    name = "clock-bound-configuration-test",
    about = "Validate ClockBound daemon configuration handling"
)]
struct Args {
    /// Path to the clockbound binary under test.
    #[clap(long, default_value = "target/release/clockbound")]
    clockbound: PathBuf,
}

/// A case, given the daemon to drive. Asserts, so it returns or it panics.
type Case = fn(&Path);

/// Every case, run in order.
const CASES: &[(&str, Case)] = &[
    (
        "missing_file_is_reported_on_stderr",
        missing_file_is_reported_on_stderr,
    ),
    (
        "invalid_syntax_points_at_the_offending_line",
        invalid_syntax_points_at_the_offending_line,
    ),
    (
        "every_semantic_violation_is_listed",
        every_semantic_violation_is_listed,
    ),
    (
        "only_the_enabled_stream_is_written",
        only_the_enabled_stream_is_written,
    ),
    (
        "no_stream_enabled_writes_no_stream_files",
        no_stream_enabled_writes_no_stream_files,
    ),
    (
        "every_stream_enabled_writes_them_all",
        every_stream_enabled_writes_them_all,
    ),
    (
        "verbose_makes_debug_records_visible",
        verbose_makes_debug_records_visible,
    ),
    (
        "debug_records_are_hidden_without_verbose",
        debug_records_are_hidden_without_verbose,
    ),
    (
        "checking_a_valid_configuration_reports_it_valid",
        checking_a_valid_configuration_reports_it_valid,
    ),
    (
        "checking_a_bad_configuration_reports_the_violation",
        checking_a_bad_configuration_reports_the_violation,
    ),
];

fn main() {
    let args = Args::parse();

    if !args.clockbound.is_file() {
        eprintln!(
            "{} is not a file. Build it with `cargo build --release --bin clockbound --features daemon`.",
            args.clockbound.display()
        );
        std::process::exit(1);
    }

    // Named before it runs, so an assertion that stops the run says which case it
    // came from.
    for (name, case) in CASES {
        println!("running {name}");
        case(&args.clockbound);
    }
    println!("\n{} cases passed", CASES.len());
}

/// Write `contents` to a new file in `dir` and return its path.
fn write_config(dir: &Path, contents: &str) -> PathBuf {
    let path = dir.join("clockbound.toml");
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(contents.as_bytes()).unwrap();
    path
}

/// Send the daemon's stdout to `path`.
fn stdout_to(path: &Path) -> Stdio {
    Stdio::from(std::fs::File::create(path).expect("failed to create stdout file"))
}

/// Whether `path` holds `needle`, counting an unreadable file as not yet.
fn contains(path: &Path, needle: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|contents| contents.contains(needle))
}

/// Run the daemon to completion, returning its exit code, stdout, and stderr. Only
/// for inputs that make it exit on its own, such as a bad configuration or a flag
/// that reports and exits.
///
/// Fails rather than waits if the daemon keeps running.
/// Both streams are read once the process is gone.
fn run_expecting_exit(clockbound: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let mut child = Command::new(clockbound)
        .args(args)
        .env_remove("RUST_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn clockbound");

    let deadline = Instant::now() + DEADLINE;
    let status = loop {
        match child.try_wait().expect("failed to wait on clockbound") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("clockbound kept running, expected it exit");
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout was piped")
        .read_to_end(&mut stdout)
        .expect("failed to read stdout");
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .expect("stderr was piped")
        .read_to_end(&mut stderr)
        .expect("failed to read stderr");
    (
        status.code(),
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    )
}

/// Start the daemon, wait until `predicate` holds (or time out), then stop it.
/// Returns whether the predicate was satisfied.
///
/// Panics if the daemon exits first, reporting its own reason.
fn run_until(
    clockbound: &Path,
    args: &[&str],
    stdout: Stdio,
    predicate: impl Fn() -> bool,
) -> bool {
    let mut child = Command::new(clockbound)
        .args(args)
        .env_remove("RUST_LOG")
        .stdout(stdout)
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn clockbound");

    let deadline = Instant::now() + DEADLINE;
    let satisfied = loop {
        // Sampled before the predicate: if the daemon is gone, whatever the
        // predicate sees next is its final word.
        let exited = child.try_wait().expect("failed to wait on clockbound");
        if predicate() {
            break true;
        }
        if let Some(status) = exited {
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            panic!("clockbound exited with {status} before {args:?} took effect: {stderr}");
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let _ = child.kill();
    let _ = child.wait();
    satisfied
}

/// Whether `predicate` ever holds while the daemon runs, counting the daemon
/// exiting as never.
///
/// Separate from `run_until`, which fails when the daemon exits first.
fn appeared_while_running(
    clockbound: &Path,
    args: &[&str],
    stdout: Stdio,
    predicate: impl Fn() -> bool,
) -> bool {
    let mut child = Command::new(clockbound)
        .args(args)
        .env_remove("RUST_LOG")
        .stdout(stdout)
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn clockbound");

    let deadline = Instant::now() + DEADLINE;
    let appeared = loop {
        if predicate() {
            break true;
        }
        if child
            .try_wait()
            .expect("failed to wait on clockbound")
            .is_some()
        {
            // still counts.
            break predicate();
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let _ = child.kill();
    let _ = child.wait();
    appeared
}

// Which configurations are rejected is covered by unit tests. What only the
// binary shows is how a rejection reaches the operator: on stderr, rendered,
// and with startup stopped.

fn missing_file_is_reported_on_stderr(clockbound: &Path) {
    let (code, _, stderr) = run_expecting_exit(clockbound, &["-c", "/nonexistent/clockbound.toml"]);
    assert_eq!(code, Some(2));
    assert_eq!(
        stderr.trim_end(),
        "Configuration file not found: /nonexistent/clockbound.toml"
    );
}

fn invalid_syntax_points_at_the_offending_line(clockbound: &Path) {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        indoc! {r"
            [logging]
            log_directory = = 1
        "},
    );

    let (code, _, stderr) = run_expecting_exit(clockbound, &["-c", path.to_str().unwrap()]);
    assert_eq!(code, Some(2));
    // The operator is pointed at the line rather than told only that it failed.
    assert!(
        stderr.contains("Invalid configuration syntax"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("line 2"), "stderr: {stderr}");
    assert!(stderr.contains("log_directory = = 1"), "stderr: {stderr}");
}

fn every_semantic_violation_is_listed(clockbound: &Path) {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        indoc! {r#"
            [logging.ffevents]
            enabled = true

            [[sources.ntp]]
            pool = "a.example"

            [[sources.ntp]]
            pool = "b.example"
        "#},
    );

    let (code, _, stderr) = run_expecting_exit(clockbound, &["-c", path.to_str().unwrap()]);
    assert_eq!(code, Some(2));
    // Two violations reported at once.
    assert!(
        stderr.starts_with("Invalid configuration:\n"),
        "stderr: {stderr}"
    );
    for expected in [
        "\n  - at most 1 `pool` entry is supported, found 2",
        "\n  - `log_directory` is required when any log stream is enabled",
    ] {
        assert!(stderr.contains(expected), "stderr: {stderr}");
    }
}

fn only_the_enabled_stream_is_written(clockbound: &Path) {
    for (enabled_key, enabled_file) in STREAMS {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        std::fs::create_dir(&logs).unwrap();

        let path = write_config(
            dir.path(),
            &format!(
                indoc! {r"
                    [logging]
                    log_directory = {:?}

                    [logging.{}]
                    enabled = true
                "},
                logs.to_str().unwrap(),
                enabled_key
            ),
        );

        let enabled = logs.join(enabled_file);
        let probe = enabled.clone();
        let created = run_until(
            clockbound,
            &["-c", path.to_str().unwrap()],
            Stdio::null(),
            move || probe.exists(),
        );
        assert!(created, "{} was not created", enabled.display());

        for (_, file_name) in STREAMS {
            let file = logs.join(file_name);
            let wanted = file_name == enabled_file;
            assert_eq!(
                file.exists(),
                wanted,
                "with only `{enabled_key}` enabled, {} should be {}",
                file.display(),
                if wanted { "written" } else { "absent" }
            );
        }
    }
}

fn no_stream_enabled_writes_no_stream_files(clockbound: &Path) {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    std::fs::create_dir(&logs).unwrap();

    let path = write_config(
        dir.path(),
        &format!(
            indoc! {r"
                [logging]
                log_directory = {:?}
            "},
            logs.to_str().unwrap()
        ),
    );

    let probe = logs.clone();
    let appeared = appeared_while_running(
        clockbound,
        &["-c", path.to_str().unwrap()],
        Stdio::null(),
        move || STREAMS.iter().any(|(_, file)| probe.join(file).exists()),
    );
    assert!(
        !appeared,
        "a stream file appeared in {} with every stream disabled",
        logs.display()
    );
}

fn every_stream_enabled_writes_them_all(clockbound: &Path) {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    std::fs::create_dir(&logs).unwrap();

    let mut contents = format!(
        indoc! {r"
            [logging]
            log_directory = {:?}
        "},
        logs.to_str().unwrap()
    );
    for (key, _) in STREAMS {
        write!(
            contents,
            indoc! {r"
                [logging.{}]
                enabled = true
            "},
            key
        )
        .expect("writing to a String cannot fail");
    }
    let path = write_config(dir.path(), &contents);

    let probe = logs.clone();
    let all_written = run_until(
        clockbound,
        &["-c", path.to_str().unwrap()],
        Stdio::null(),
        move || STREAMS.iter().all(|(_, file)| probe.join(file).exists()),
    );
    assert!(
        all_written,
        "not every stream file appeared in {}",
        logs.display()
    );
}

fn checking_a_valid_configuration_reports_it_valid(clockbound: &Path) {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        indoc! {r#"
            [logging]
            log_directory = "/var/log/clockbound"

            [logging.synchronization]
            enabled = true

            [[sources.ntp]]
            server = "192.0.2.1"

            [[sources.ntp]]
            pool = "pool.ntp.example"
        "#},
    );

    let (code, stdout, stderr) = run_expecting_exit(
        clockbound,
        &["--check-configuration", "-c", path.to_str().unwrap()],
    );
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert!(stdout.contains("is valid"), "stdout: {stdout}");
}

fn checking_a_bad_configuration_reports_the_violation(clockbound: &Path) {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        "[[sources.ntp]]\npool = \"a.example\"\n\n[[sources.ntp]]\npool = \"b.example\"\n",
    );

    let (code, stdout, stderr) = run_expecting_exit(
        clockbound,
        &["--check-configuration", "-c", path.to_str().unwrap()],
    );
    assert_eq!(code, Some(2));
    assert!(
        stderr.contains("at most 1 `pool` entry is supported, found 2"),
        "stderr: {stderr}"
    );
    assert!(!stdout.contains("is valid"), "stdout: {stdout}");
}

// `-v` is only observable from outside.

fn verbose_makes_debug_records_visible(clockbound: &Path) {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("stdout.log");
    let probe = out.clone();

    let seen = run_until(clockbound, &["-v"], stdout_to(&out), move || {
        contains(&probe, "DEBUG")
    });
    assert!(seen, "-v did not make debug records visible");
}

fn debug_records_are_hidden_without_verbose(clockbound: &Path) {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("stdout.log");
    let probe = out.clone();

    let appeared = appeared_while_running(clockbound, &[], stdout_to(&out), move || {
        contains(&probe, "DEBUG")
    });
    assert!(
        !appeared,
        "debug records appeared without -v: {}",
        std::fs::read_to_string(&out).unwrap_or_default()
    );
}
