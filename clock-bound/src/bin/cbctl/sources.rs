//! `cbctl sources` — display the state of the time sources configured with the ClockBound daemon.
//!
//! ```text
//! cbctl sources [--log-dir DIR]
//! ```
//!
//! The command reads the most recent snapshot (the last line) of the daemon's `sources.log` and
//! renders it as a table, followed by a footer with the snapshot timestamp. There is no IPC with
//! the daemon: output is only as fresh as the last snapshot, and sources logging must be enabled
//! in the daemon configuration. A warning is printed when the snapshot is older than two
//! snapshot periods, which suggests the daemon has stopped logging.
//!
//! Exit codes:
//! - 0: the snapshot was read and displayed (even if stale).
//! - 1: `sources.log` could not be read, or its latest snapshot could not be parsed.

use std::fs::File;
use std::io::{self, BufReader};
use std::iter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{DateTime, Utc};
use clap::Args;
use clock_bound::daemon::logging::sources::{
    Health, SNAPSHOT_PERIOD_SECS, SourceRow, SourceStatus, SourcesSnapshot, StatSummaryNs,
};
use clock_bound::daemon::logging::{SourceType, StructuredLog};
use serde::Deserialize;

/// Exit code when the snapshot was read and displayed.
const EXIT_CODE_SUCCESS: u8 = 0;

/// Exit code when `sources.log` could not be read or parsed.
const EXIT_CODE_ERROR: u8 = 1;

/// Directories searched, in order, for `sources.log` when `--log-dir` is not given. The first
/// directory containing the file is used.
const DEFAULT_LOG_DIRS: [&str; 1] = ["/var/log/clockbound"];

/// A snapshot older than this many seconds is reported as stale.
const STALE_SNAPSHOT_SECS: i64 = 2 * SNAPSHOT_PERIOD_SECS.cast_signed();

/// Rendering of an absent or null value.
const NULL_VALUE: &str = "—";

/// Number of columns in the table.
const COLUMN_COUNT: usize = 8;

/// The table columns, in display order. Numeric columns are right-aligned, text columns are
/// left-aligned.
const COLUMNS: [Column; COLUMN_COUNT] = [
    Column::new("SOURCE", Align::Left),
    Column::new("TYPE", Align::Left),
    Column::new("STATUS", Align::Left),
    Column::new("SELECTED", Align::Left),
    Column::new("LATENCY", Align::Right),
    Column::new("SOURCE_CLOCK_ERROR_BOUND", Align::Right),
    Column::new("POLL", Align::Right),
    Column::new("RESPONSES/REQUESTS", Align::Left),
];

/// Display the state of the time sources configured with the ClockBound daemon.
#[derive(Debug, Args)]
pub struct SourcesArgs {
    /// Directory containing the ClockBound daemon `sources.log` file. Defaults to searching
    /// `/var/log/clockbound`.
    #[arg(long, value_name = "DIR")]
    pub log_dir: Option<PathBuf>,
}

/// One line of `sources.log`: the event envelope around a [`SourcesSnapshot`]. Only the fields
/// rendered by the command are deserialized; other fields are ignored.
#[derive(Debug, Deserialize)]
struct SnapshotLine {
    /// ISO 8601 UTC timestamp of when the snapshot was taken.
    timestamp: String,
    sources_snapshot: SourcesSnapshot,
}

/// A parsed snapshot, ready to render.
#[derive(Debug)]
struct Snapshot {
    timestamp: DateTime<Utc>,
    sources: Vec<SourceRow>,
}

/// Horizontal alignment of a table column.
#[derive(Debug, Clone, Copy)]
enum Align {
    Left,
    Right,
}

/// A table column: its header and the alignment of its cells.
#[derive(Debug)]
struct Column {
    header: &'static str,
    align: Align,
}

impl Column {
    const fn new(header: &'static str, align: Align) -> Self {
        Column { header, align }
    }
}

/// One table row of formatted cells, in [`COLUMNS`] order.
type Row = [String; COLUMN_COUNT];

/// Resolve the path to `sources.log`: under `--log-dir` if given, otherwise under the first
/// default directory that contains it. Returns `None` if `sources.log` is not found.
fn resolve_log_path(log_dir: Option<&Path>) -> Option<PathBuf> {
    let file_name = StructuredLog::Sources.file_name();
    if let Some(dir) = log_dir {
        return Some(dir.join(file_name)).filter(|path| path.exists());
    }
    DEFAULT_LOG_DIRS
        .iter()
        .map(|dir| Path::new(dir).join(file_name))
        .find(|path| path.exists())
}

/// Parse an ISO 8601 / RFC 3339 timestamp from the log into a UTC instant.
fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|error| format!("invalid timestamp {value:?}: {error}"))
}

/// Describe an I/O error for the "unable to read" message.
fn describe_io_error(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => String::from("file not found"),
        io::ErrorKind::PermissionDenied => String::from("permission denied"),
        _ => error.to_string(),
    }
}

/// Read and parse the most recent snapshot (the last non-empty line) of `sources.log`.
///
/// Errors carry the full human readable message to print.
fn read_latest_snapshot(path: &Path) -> Result<Snapshot, String> {
    use std::io::BufRead;

    let unable_to_read = |error: &io::Error| {
        format!(
            "error: unable to read sources.log ({}): {}\n\
             Check that the clockbound daemon is running and that sources logs are enabled.\n\
             If log directory is configured, execute this command with `--log-dir <DIR>`.",
            path.display(),
            describe_io_error(error)
        )
    };

    let file = File::open(path).map_err(|error| unable_to_read(&error))?;
    let mut last_line: Option<String> = None;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| unable_to_read(&error))?;
        if !line.trim().is_empty() {
            last_line = Some(line);
        }
    }
    let Some(last_line) = last_line else {
        return Err(format!(
            "error: no snapshot found in sources.log ({})\n\
             Check that the clockbound daemon is running and that sources logs are enabled.",
            path.display()
        ));
    };

    parse_snapshot(&last_line).map_err(|reason| {
        format!(
            "error: unable to parse the latest snapshot in sources.log ({}): {reason}",
            path.display()
        )
    })
}

/// Parse one `sources.log` line into a [`Snapshot`].
fn parse_snapshot(line: &str) -> Result<Snapshot, String> {
    let parsed: SnapshotLine = serde_json::from_str(line).map_err(|error| error.to_string())?;
    Ok(Snapshot {
        timestamp: parse_timestamp(&parsed.timestamp)?,
        sources: parsed.sources_snapshot.sources,
    })
}

/// Format a nanosecond value in its natural unit (ns, us, ms, s), rounded to three significant
/// figures with trailing zeros dropped. For example `412 ns`, `2.5 us`, `30.1 us`, `141 us`,
/// `2.15 ms`. Values below 1 us are shown as exact nanoseconds, and values of 1000 s or more
/// are rounded to whole seconds (e.g. `5000 s`).
fn format_nanos(nanos: i64) -> String {
    const SUBSECOND_UNITS: [(f64, &str); 2] = [(1e3, "us"), (1e6, "ms")];

    if nanos.unsigned_abs() < 1_000 {
        return format!("{nanos} ns");
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "only three significant figures are displayed"
    )]
    let nanos = nanos as f64;
    for (scale, unit) in SUBSECOND_UNITS {
        let (rounded, decimals) = round_to_three_significant_figures(nanos / scale);
        // Rounding can carry to 1000 (e.g. 999.6 us), which is displayed in the next unit up.
        if rounded.abs() < 1000.0 {
            return format_decimal(rounded, decimals, unit);
        }
    }
    let (rounded, decimals) = round_to_three_significant_figures(nanos / 1e9);
    format_decimal(rounded, decimals, "s")
}

/// Round `value`, expected to be at least 1, to three significant figures. Returns the rounded
/// value and the number of decimals needed to display it.
fn round_to_three_significant_figures(value: f64) -> (f64, usize) {
    let (decimals, factor) = match value.abs() {
        magnitude if magnitude >= 100.0 => (0, 1.0),
        magnitude if magnitude >= 10.0 => (1, 10.0),
        _ => (2, 100.0),
    };
    ((value * factor).round() / factor, decimals)
}

/// Format `value` with `decimals` decimals, trailing zeros dropped, followed by `unit`:
/// `2.50` becomes `2.5 us`, `21.0` becomes `21 us`.
fn format_decimal(value: f64, decimals: usize, unit: &str) -> String {
    let digits = format!("{value:.decimals$}");
    let digits = if decimals > 0 {
        digits.trim_end_matches('0').trim_end_matches('.')
    } else {
        &digits
    };
    format!("{digits} {unit}")
}

/// Format an age or duration in seconds: `42s`, `4m`, `1h 12m`, `1d 4h`. Units below the
/// displayed ones are rounded down. Negative ages (e.g. a timestamp slightly in the future)
/// render as `0s`.
fn format_age(seconds: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;

    let seconds = seconds.max(0);
    if seconds < MINUTE {
        format!("{seconds}s")
    } else if seconds < HOUR {
        format!("{}m", seconds / MINUTE)
    } else if seconds < DAY {
        format!("{}h {}m", seconds / HOUR, seconds % HOUR / MINUTE)
    } else {
        format!("{}d {}h", seconds / DAY, seconds % DAY / HOUR)
    }
}

/// Format a poll interval in seconds: `2s`, `0.5s`, `16s`.
fn format_poll(seconds: f64) -> String {
    format!("{seconds}s")
}

/// Format a source type as named in `sources.log`.
fn format_source_type(source_type: SourceType) -> &'static str {
    match source_type {
        SourceType::Ntp => "ntp",
        SourceType::Phc => "phc",
    }
}

/// Format a source status as named in `sources.log`.
fn format_status(status: SourceStatus) -> &'static str {
    match status {
        SourceStatus::Unknown => "UNKNOWN",
        SourceStatus::Synchronized => "SYNCHRONIZED",
        SourceStatus::FreeRunning => "FREE_RUNNING",
    }
}

/// Format a source's selection history as of `now`:
/// - `selected` if currently selected (selected, and not deselected since).
/// - `<age> ago` if its last selection tenure ended `<age>` before `now`.
/// - `never` if it has never been selected.
fn format_selected(health: &Health, now: DateTime<Utc>) -> Result<String, String> {
    match (&health.selected_at, &health.deselected_at) {
        (_, Some(deselected_at)) => {
            let deselected_at = parse_timestamp(deselected_at)?;
            let age = format_age((now - deselected_at).num_seconds());
            Ok(format!("{age} ago"))
        }
        (Some(_), None) => Ok(String::from("selected")),
        (None, None) => Ok(String::from("never")),
    }
}

/// Format an optional stat summary's median in its natural unit, or the null value.
fn format_median(summary: Option<&StatSummaryNs>) -> String {
    summary.map_or_else(
        || String::from(NULL_VALUE),
        |summary| format_nanos(summary.median),
    )
}

/// Format a source into one table row.
fn format_row(source: &SourceRow, now: DateTime<Utc>) -> Result<Row, String> {
    Ok([
        source.identifier.clone(),
        String::from(format_source_type(source.source_type)),
        String::from(format_status(source.health.status)),
        format_selected(&source.health, now)?,
        format_median(source.stats.latency_ns.as_ref()),
        format_median(source.stats.source_clock_error_bound_ns.as_ref()),
        format_poll(source.poll_interval_sec),
        format!("{}/{}", source.stats.responses, source.stats.requests),
    ])
}

/// Render the full command output for `snapshot` as of `now`: the table, the snapshot footer,
/// and a stale-snapshot warning if applicable.
///
/// Every row is formatted before anything is rendered, so a malformed source fails the whole
/// command rather than printing a partial table.
fn render(snapshot: &Snapshot, now: DateTime<Utc>) -> Result<String, String> {
    use std::fmt::Write;

    /// Panic message for writes to a `String`, which cannot fail.
    const WRITE_TO_STRING: &str = "writing to a String cannot fail";

    let header: Row = COLUMNS.map(|column| String::from(column.header));
    let rows: Vec<Row> = snapshot
        .sources
        .iter()
        .map(|source| format_row(source, now))
        .collect::<Result<_, _>>()?;

    // Each column is as wide as its widest cell (header included), measured in characters.
    let mut widths: [usize; COLUMN_COUNT] = [0; COLUMN_COUNT];
    let all_rows = iter::once(&header).chain(&rows);
    for row in all_rows.clone() {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }

    let mut output = String::new();
    for row in all_rows {
        let mut line = String::new();
        for (index, ((cell, width), column)) in row.iter().zip(widths).zip(&COLUMNS).enumerate() {
            // Two spaces between columns, none before the first, so the table starts at the
            // beginning of the line.
            if index > 0 {
                line.push_str("  ");
            }
            match column.align {
                Align::Left => write!(line, "{cell:<width$}"),
                Align::Right => write!(line, "{cell:>width$}"),
            }
            .expect(WRITE_TO_STRING);
        }
        writeln!(output, "{}", line.trim_end()).expect(WRITE_TO_STRING);
    }

    let age_seconds = (now - snapshot.timestamp).num_seconds();
    let age = format_age(age_seconds);
    writeln!(
        output,
        "\n[ Snapshot from {} ({age} ago) ]",
        snapshot.timestamp.format("%Y-%m-%dT%H:%M:%SZ")
    )
    .expect(WRITE_TO_STRING);
    if age_seconds > STALE_SNAPSHOT_SECS {
        writeln!(
            output,
            "[ WARNING: snapshot is {age} old - clockbound daemon appears to have stopped logging ]"
        )
        .expect(WRITE_TO_STRING);
    }
    Ok(output)
}

/// Run the `sources` command.
pub fn run(args: &SourcesArgs) -> ExitCode {
    let Some(path) = resolve_log_path(args.log_dir.as_deref()) else {
        let searched = args.log_dir.as_ref().map_or_else(
            || DEFAULT_LOG_DIRS.join(", "),
            |dir| dir.display().to_string(),
        );
        eprintln!(
            "error: unable to find sources.log in {searched}\n\
             Check that the clockbound daemon is running and that sources logs are enabled.\n\
             If log directory is configured, execute this command with `--log-dir <DIR>`."
        );
        return ExitCode::from(EXIT_CODE_ERROR);
    };
    let output = read_latest_snapshot(&path).and_then(|snapshot| render(&snapshot, Utc::now()));
    match output {
        Ok(output) => {
            print!("{output}");
            ExitCode::from(EXIT_CODE_SUCCESS)
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(EXIT_CODE_ERROR)
        }
    }
}

#[cfg(test)]
mod test {
    use tempfile::TempDir;

    use super::*;

    /// A snapshot line matching the documented `sources.log` schema, with one source of each
    /// selection state and status.
    const SNAPSHOT_LINE: &str = r#"{"timestamp":"2026-07-16T14:34:00Z","event":"sources_snapshot","sources_snapshot":{"sources":[{"type":"ntp","identifier":"169.254.169.123:123","poll_interval_sec":2.0,"health":{"selected_at":"2026-07-16T13:22:01Z","deselected_at":null,"status":"SYNCHRONIZED","last_response_at":"2026-07-16T14:33:58Z"},"stats":{"requests":512,"responses":508,"latency_ns":{"min":128400,"median":141200,"max":155000},"source_clock_error_bound_ns":{"min":27800,"median":30100,"max":31200},"window_duration_sec":1024}},{"type":"phc","identifier":"/dev/ptp0","poll_interval_sec":0.5,"health":{"selected_at":"2026-07-16T12:00:00Z","deselected_at":"2026-07-16T13:22:00Z","status":"SYNCHRONIZED","last_response_at":"2026-07-16T14:33:59Z"},"stats":{"requests":2048,"responses":2048,"latency_ns":{"min":2400,"median":2500,"max":2600},"source_clock_error_bound_ns":{"min":20000,"median":21000,"max":22000},"window_duration_sec":1024}},{"type":"ntp","identifier":"166.117.111.42:123","poll_interval_sec":16.0,"health":{"selected_at":null,"deselected_at":null,"status":"FREE_RUNNING","last_response_at":"2026-07-16T14:33:50Z"},"stats":{"requests":64,"responses":59,"latency_ns":{"min":2000000,"median":2150000,"max":2300000},"source_clock_error_bound_ns":{"min":1000000,"median":1120000,"max":1200000},"window_duration_sec":1024}},{"type":"ntp","identifier":"3.33.186.244:123","poll_interval_sec":16.0,"health":{"selected_at":null,"deselected_at":null,"status":"UNKNOWN","last_response_at":null},"stats":{"requests":64,"responses":0,"latency_ns":null,"source_clock_error_bound_ns":null,"window_duration_sec":1024}}]},"version":"3.0.1"}"#;

    /// The expected rendering of [`SNAPSHOT_LINE`] 21 seconds after the snapshot.
    const EXPECTED_OUTPUT: &str = "\
SOURCE               TYPE  STATUS        SELECTED    LATENCY  SOURCE_CLOCK_ERROR_BOUND  POLL  RESPONSES/REQUESTS
169.254.169.123:123  ntp   SYNCHRONIZED  selected     141 us                   30.1 us    2s  508/512
/dev/ptp0            phc   SYNCHRONIZED  1h 12m ago   2.5 us                     21 us  0.5s  2048/2048
166.117.111.42:123   ntp   FREE_RUNNING  never       2.15 ms                   1.12 ms   16s  59/64
3.33.186.244:123     ntp   UNKNOWN       never             —                         —   16s  0/64

[ Snapshot from 2026-07-16T14:34:00Z (21s ago) ]
";

    /// Parse `timestamp`, for use as the "now" of a test.
    fn at(timestamp: &str) -> DateTime<Utc> {
        parse_timestamp(timestamp).unwrap()
    }

    /// Write `contents` to a `sources.log` in a fresh temporary directory.
    fn write_log(contents: &str) -> (TempDir, PathBuf) {
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sources.log");
        File::create(&path)
            .unwrap()
            .write_all(contents.as_bytes())
            .unwrap();
        (dir, path)
    }

    /// Assert that the documented snapshot renders as the documented table and footer.
    #[test]
    fn render_documented_snapshot() {
        let snapshot = parse_snapshot(SNAPSHOT_LINE).unwrap();
        let output = render(&snapshot, at("2026-07-16T14:34:21Z")).unwrap();
        assert_eq!(output, EXPECTED_OUTPUT);
    }

    /// Assert that a snapshot older than two periods gets the stale warning after the footer.
    #[test]
    fn render_stale_snapshot_appends_warning() {
        let snapshot = parse_snapshot(SNAPSHOT_LINE).unwrap();
        let output = render(&snapshot, at("2026-07-16T14:59:00Z")).unwrap();
        assert!(
            output.ends_with(
                "\n[ Snapshot from 2026-07-16T14:34:00Z (25m ago) ]\n\
                 [ WARNING: snapshot is 25m old - clockbound daemon appears to have stopped logging ]\n"
            ),
            "expected the footer followed by the stale warning, got:\n{output}"
        );
    }

    /// Assert that a snapshot exactly two periods old is not yet stale.
    #[test]
    fn render_snapshot_at_stale_threshold_has_no_warning() {
        let snapshot = parse_snapshot(SNAPSHOT_LINE).unwrap();
        let output = render(&snapshot, at("2026-07-16T14:36:00Z")).unwrap();
        assert!(
            !output.contains("WARNING"),
            "expected no stale warning, got:\n{output}"
        );
    }

    /// Assert that a snapshot with no sources still renders the header and footer.
    #[test]
    fn render_empty_snapshot() {
        let snapshot = parse_snapshot(
            r#"{"timestamp":"2026-07-16T14:34:00Z","event":"sources_snapshot","sources_snapshot":{"sources":[]},"version":"3.0.1"}"#,
        )
        .unwrap();
        let output = render(&snapshot, at("2026-07-16T14:34:05Z")).unwrap();
        assert_eq!(
            output,
            "SOURCE  TYPE  STATUS  SELECTED  LATENCY  SOURCE_CLOCK_ERROR_BOUND  POLL  RESPONSES/REQUESTS\n\
             \n[ Snapshot from 2026-07-16T14:34:00Z (5s ago) ]\n"
        );
    }

    /// Assert that the last non-empty line of the file is the snapshot that gets read.
    #[test]
    fn read_latest_snapshot_uses_last_line() {
        let older = SNAPSHOT_LINE.replace("2026-07-16T14:34:00Z", "2026-07-16T14:33:00Z");
        let (_dir, path) = write_log(&format!("{older}\n{SNAPSHOT_LINE}\n\n"));
        let snapshot = read_latest_snapshot(&path).unwrap();
        assert_eq!(snapshot.timestamp, at("2026-07-16T14:34:00Z"));
        assert_eq!(snapshot.sources.len(), 4);
    }

    /// Assert that a missing file reports the documented "unable to read" error.
    #[test]
    fn read_latest_snapshot_missing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sources.log");
        let error = read_latest_snapshot(&path).unwrap_err();
        assert_eq!(
            error,
            format!(
                "error: unable to read sources.log ({}): file not found\n\
                 Check that the clockbound daemon is running and that sources logs are enabled.\n\
                 If log directory is configured, execute this command with `--log-dir <DIR>`.",
                path.display()
            )
        );
    }

    /// Assert that an empty file is an error.
    #[test]
    fn read_latest_snapshot_empty_file() {
        let (_dir, path) = write_log("\n");
        let error = read_latest_snapshot(&path).unwrap_err();
        assert!(
            error.starts_with("error: no snapshot found in sources.log"),
            "unexpected error: {error}"
        );
    }

    /// Assert that a latest line that does not match the schema is an error, even if older
    /// lines are valid.
    #[test]
    fn read_latest_snapshot_malformed_last_line() {
        let (_dir, path) = write_log(&format!("{SNAPSHOT_LINE}\n{{\"timestamp\":\"x\"}}\n"));
        let error = read_latest_snapshot(&path).unwrap_err();
        assert!(
            error.starts_with("error: unable to parse the latest snapshot in sources.log"),
            "unexpected error: {error}"
        );
    }

    /// Assert that a missing required field fails the parse.
    #[test]
    fn parse_snapshot_rejects_missing_required_field() {
        let line = SNAPSHOT_LINE.replacen(r#""poll_interval_sec":2.0,"#, "", 1);
        let _ = parse_snapshot(&line).unwrap_err();
    }

    /// Assert that an unknown status value fails the parse.
    #[test]
    fn parse_snapshot_rejects_unknown_status() {
        let line = SNAPSHOT_LINE.replacen("FREE_RUNNING", "BOGUS", 1);
        let _ = parse_snapshot(&line).unwrap_err();
    }

    /// Assert that an invalid snapshot timestamp fails the parse.
    #[test]
    fn parse_snapshot_rejects_invalid_timestamp() {
        let line = SNAPSHOT_LINE.replacen("2026-07-16T14:34:00Z", "yesterday", 1);
        let _ = parse_snapshot(&line).unwrap_err();
    }

    /// Assert that an invalid `deselected_at` timestamp fails rendering.
    #[test]
    fn render_rejects_invalid_deselected_at() {
        let line = SNAPSHOT_LINE.replacen("2026-07-16T13:22:00Z", "yesterday", 1);
        let snapshot = parse_snapshot(&line).unwrap();
        let _ = render(&snapshot, at("2026-07-16T14:34:21Z")).unwrap_err();
    }

    /// Assert that `--log-dir` resolves to its `sources.log` when the file exists.
    #[test]
    fn resolve_log_path_uses_log_dir() {
        let (dir, path) = write_log(SNAPSHOT_LINE);
        assert_eq!(resolve_log_path(Some(dir.path())), Some(path));
    }

    /// Assert that `--log-dir` resolves to `None` when it has no `sources.log`.
    #[test]
    fn resolve_log_path_log_dir_missing() {
        let dir = TempDir::new().unwrap();
        assert_eq!(resolve_log_path(Some(dir.path())), None);
    }

    /// Assert that without `--log-dir`, the default directory is used only if it has `sources.log`.
    #[test]
    fn resolve_log_path_defaults() {
        let expected = PathBuf::from("/var/log/clockbound/sources.log");
        assert_eq!(
            resolve_log_path(None),
            expected.exists().then_some(expected)
        );
    }

    /// Assert that nanosecond values are scaled to their natural unit at three significant
    /// figures, with trailing zeros dropped.
    #[test]
    fn format_nanos_scales_units() {
        assert_eq!(format_nanos(0), "0 ns");
        assert_eq!(format_nanos(412), "412 ns");
        assert_eq!(format_nanos(999), "999 ns");
        assert_eq!(format_nanos(1_000), "1 us");
        assert_eq!(format_nanos(2_500), "2.5 us");
        assert_eq!(format_nanos(30_100), "30.1 us");
        assert_eq!(format_nanos(141_200), "141 us");
        assert_eq!(format_nanos(2_150_000), "2.15 ms");
        assert_eq!(format_nanos(1_200_000_000), "1.2 s");
        assert_eq!(format_nanos(12_345_000_000), "12.3 s");
        assert_eq!(format_nanos(5_000_000_000_000), "5000 s");
    }

    /// Assert that rounding up to 1000 moves to the next unit.
    #[test]
    fn format_nanos_rounding_carries_into_next_unit() {
        assert_eq!(format_nanos(999_600), "1 ms");
        assert_eq!(format_nanos(999_600_000), "1 s");
    }

    /// Assert that the extremes of `i64` format in whole seconds without overflowing.
    #[test]
    fn format_nanos_extremes() {
        assert_eq!(format_nanos(i64::MAX), "9223372037 s");
        assert_eq!(format_nanos(i64::MIN), "-9223372037 s");
    }

    /// Assert that ages are rendered with the documented granularity.
    #[test]
    fn format_age_granularity() {
        assert_eq!(format_age(-5), "0s");
        assert_eq!(format_age(0), "0s");
        assert_eq!(format_age(42), "42s");
        assert_eq!(format_age(4 * 60 + 59), "4m");
        assert_eq!(format_age(60 * 60 + 12 * 60 + 30), "1h 12m");
        assert_eq!(format_age(28 * 60 * 60 + 30 * 60), "1d 4h");
    }

    /// Assert that poll intervals drop a zero fractional part.
    #[test]
    fn format_poll_values() {
        assert_eq!(format_poll(2.0), "2s");
        assert_eq!(format_poll(0.5), "0.5s");
        assert_eq!(format_poll(16.0), "16s");
    }

    /// Assert that each source type renders as named in `sources.log`.
    #[test]
    fn format_source_type_values() {
        assert_eq!(format_source_type(SourceType::Ntp), "ntp");
        assert_eq!(format_source_type(SourceType::Phc), "phc");
    }

    /// Assert that each status renders as named in `sources.log`.
    #[test]
    fn format_status_values() {
        assert_eq!(format_status(SourceStatus::Unknown), "UNKNOWN");
        assert_eq!(format_status(SourceStatus::Synchronized), "SYNCHRONIZED");
        assert_eq!(format_status(SourceStatus::FreeRunning), "FREE_RUNNING");
    }

    /// Assert the selection history rendering for each combination of timestamps.
    #[test]
    fn format_selected_values() {
        let now = at("2026-07-16T14:34:00Z");
        let health = |selected_at: Option<&str>, deselected_at: Option<&str>| Health {
            selected_at: selected_at.map(String::from),
            deselected_at: deselected_at.map(String::from),
            status: SourceStatus::Unknown,
        };
        assert_eq!(
            format_selected(&health(Some("2026-07-16T13:00:00Z"), None), now).unwrap(),
            "selected"
        );
        assert_eq!(
            format_selected(
                &health(Some("2026-07-16T12:00:00Z"), Some("2026-07-16T13:22:00Z")),
                now
            )
            .unwrap(),
            "1h 12m ago"
        );
        assert_eq!(format_selected(&health(None, None), now).unwrap(), "never");
    }
}
