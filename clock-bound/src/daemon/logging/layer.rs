//! Custom tracing [`Layer`] for structured log files.
//!
//! Provides [`StructuredLogLayer`], which writes newline-delimited JSON to a file
//! with automatic injection of `timestamp` and `version` fields.
//!
//! Note that there are two distinct use case defining the type of timestamp
//! offered by a  [`TimestampSource`]:
//! - the wall clock for the daemon at the time events are processed
//! - a fixed value for a replay whose output must be traceable and reproducible

use std::{
    fmt,
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use serde_json::value::RawValue;
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id},
    warn,
};
use tracing_subscriber::{layer::Context, registry::LookupSpan};

use super::{StructuredLog, format_log_timestamp};

/// The log file behind a buffer.
///
/// Lines are buffered (`BufWriter`'s default, 8 KiB) and reach the file in blocks: one `write(2)`
/// per block instead of one per line. On the daemon's streams that is at most a few seconds of
/// lines in memory. On a trace replay, with millions of lines, buffering the output cuts down
/// execution time.
///
/// The buffer is flushed before the file is swapped on [`LogHandle::reopen`], and by
/// [`LogHandle::flush`] when the process is done writing. Lines still buffered when the process
/// aborts are lost. Similar contract with unbuffered writer, which would loose the last logs.
type LogWriter = BufWriter<File>;

/// A handle to a structured log file that supports reopening.
///
/// Used to implement lossless log rotation: an external tool (e.g., logrotate)
/// renames the current file, then sends SIGHUP to trigger [`LogHandle::reopen`],
/// which flushes and closes the old fd and opens a new file at the original path.
pub struct LogHandle {
    writer: Arc<Mutex<Option<LogWriter>>>,
    path: PathBuf,
}

impl LogHandle {
    /// Write out the lines still in the buffer.
    ///
    /// The layers are owned by the global subscriber, which is never dropped, so the buffer is not
    /// flushed by going out of scope: call this when the process is done writing (the end of a
    /// replay, the daemon's shutdown). A flush error is reported, as in `reopen`.
    pub(super) fn flush(&self) {
        if let Ok(mut guard) = self.writer.lock()
            && let Some(ref mut w) = *guard
            && let Err(e) = w.flush()
        {
            warn!("Failed to flush log file {}: {e}", self.path.display());
        }
    }

    /// Close and reopen the log file at its original path.
    ///
    /// The old writer is taken out from under the lock and the new one put in its place; the old
    /// one is then flushed and synced outside the lock. A line written between logrotate's rename
    /// and the signal is still in the old buffer and lands in the renamed file, where it belongs.
    ///
    /// On failure, logs an error to stderr and drops the writer so the old fd
    /// is closed (freeing disk space). A subsequent successful `reopen` call
    /// will restore logging.
    pub(super) fn reopen(&self) {
        let new_writer = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(file) => Some(BufWriter::new(file)),
            Err(e) => {
                warn!(
                    "Failed to reopen log file {}: {e}. Suspending output to this log.",
                    self.path.display()
                );
                None
            }
        };

        let mut guard = self.writer.lock().unwrap();
        let old_writer = guard.take();

        *guard = new_writer;
        drop(guard);

        if let Some(old_writer) = old_writer {
            // `into_inner` flushes the buffer and hands back the file; a flush error is reported
            // rather than swallowed, and the file is closed either way when it goes out of scope.
            match old_writer.into_inner() {
                Ok(old_log_file) => {
                    if let Err(e) = old_log_file.sync_all() {
                        warn!(
                            "Failed to sync log file {} before reopen: {e}",
                            self.path.display()
                        );
                    }
                }
                Err(e) => warn!(
                    "Failed to flush log file {} before reopen: {e}",
                    self.path.display()
                ),
            }
        }
    }
}

/// Where the `timestamp` injected into every log line comes from.
///
/// The daemon records when each line was written, as it should. A replay of a recorded trace
/// (`clock-bound-ff-tester repro-replay`), however, is a pure function of its inputs, and the same
/// input should lead to the same output; a wall-clock stamp on every line would defeat that.
/// The mechanism chosen here is to maintain the logging format, but for the replay to use a fixed
/// value instead of wallclock time. Instead of the replay passing through timestamps "at the time
/// of the capture" the fixed value unambiguously distinguishes a replayed trace from a daemon capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampSource {
    /// `Utc::now()` at the time each line is written.
    WallClock,
    /// The Unix epoch (`1970-01-01T00:00:00.000000Z`) on every line: unmistakably not a
    /// real time, and the same on every host.
    Fixed,
    /// No `timestamp` field at all, for a stream whose records carry their own time reference
    /// (the `algo_analysis` records are joined to the feed on `seq`).
    None,
}

impl TimestampSource {
    /// The timestamp to stamp a line with, if any.
    fn now(self) -> Option<DateTime<Utc>> {
        match self {
            Self::WallClock => Some(Utc::now()),
            Self::Fixed => Some(DateTime::UNIX_EPOCH),
            Self::None => None,
        }
    }
}

/// A tracing [`Layer`] that writes pre-serialized JSON entries as newline-delimited
/// JSON to a file, injecting `timestamp` and `version` fields automatically.
///
/// Events are filtered by tracing target prefix. The layer expects an `entry` field
/// containing a JSON-serialized string.
///
/// This layer/target is not intended to be called into directly by the main app,
/// but via strongly-typed helper libraries.
pub struct StructuredLogLayer {
    writer: Arc<Mutex<Option<LogWriter>>>,
    target_prefix: &'static str,
    version: &'static str,
    timestamp_source: TimestampSource,
}

impl StructuredLogLayer {
    /// Create a new `StructuredLogLayer` that writes to the specified file.
    ///
    /// Returns the layer and a [`LogHandle`] that can be used to reopen the file
    /// (e.g., on SIGHUP for log rotation).
    ///
    /// # Arguments
    /// - `log_directory` - Directory where the log file will be created
    /// - `filename` - Name of the log file (e.g., `"synchronization.log"`)
    /// - `target_prefix` - Tracing target prefix to filter events (e.g., `"clock_bound::synchronization"`)
    /// - `version` - Version string to inject into every log entry
    /// - `timestamp_source` - Where the `timestamp` injected into every log entry comes from
    ///
    /// # Panics
    /// Panics if the log file cannot be opened or created at `log_directory/filename`.
    fn new(
        log_directory: impl AsRef<Path>,
        filename: &str,
        target_prefix: &'static str,
        version: &'static str,
        timestamp_source: TimestampSource,
    ) -> (Self, LogHandle) {
        let dir = log_directory.as_ref();
        std::fs::create_dir_all(dir)
            .unwrap_or_else(|e| panic!("Failed to create log directory {}: {}", dir.display(), e));
        let path = dir.join(filename);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("Failed to open log file {}: {}", path.display(), e));

        let writer = Arc::new(Mutex::new(Some(BufWriter::new(file))));

        let layer = Self {
            writer: Arc::clone(&writer),
            target_prefix,
            version,
            timestamp_source,
        };

        let handle = LogHandle { writer, path };

        (layer, handle)
    }

    /// Convenience constructor that derives the log file name and target prefix
    /// from a [`StructuredLog`] stream instead of taking raw strings.
    ///
    /// Delegates to [`new`](StructuredLogLayer::new); see it for panic behavior.
    pub fn for_stream(
        log_directory: impl AsRef<Path>,
        stream: StructuredLog,
        version: &'static str,
        timestamp_source: TimestampSource,
    ) -> (Self, LogHandle) {
        Self::new(
            log_directory,
            stream.file_name(),
            stream.target(),
            version,
            timestamp_source,
        )
    }
}

impl<S> tracing_subscriber::Layer<S> for StructuredLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    /// Remember a span's `seq` field so that records emitted inside it can carry it (see
    /// `on_event`). Only spans on this layer's own target: a span is one object seen by every layer,
    /// and only the stream the span names stamps its records.
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if !attrs.metadata().target().starts_with(self.target_prefix) {
            return;
        }
        let mut visitor = SeqVisitor::default();
        attrs.record(&mut visitor);
        if let (Some(seq), Some(span)) = (visitor.seq, ctx.span(id)) {
            // `replace`, not `insert`: another layer on the same target may have stored it first.
            span.extensions_mut().replace(SpanSeq(seq));
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if !event.metadata().target().starts_with(self.target_prefix) {
            return;
        }

        let mut visitor = EntryVisitor::default();
        event.record(&mut visitor);

        let Some(entry_json) = visitor.entry else {
            return;
        };

        // The entry is one JSON object, serialised by the emitting code (`serde_json::to_string` on a
        // typed record). The line written is that object with three fields added around and inside
        // it: `{"timestamp":…,"event":…,"seq":…,<the entry's other members>,"version":…}`. Nothing in
        // the entry's *values* is needed to do this — only the member boundaries and the keys — so the
        // values are not parsed into `serde_json::Value`s and re-serialised; they are passed through
        // as the text they arrived as.
        //
        // `RawValue` is serde_json's type for exactly that: the unparsed text of one JSON value,
        // borrowed from the input. `parse_json_members` still runs serde_json's parser over the whole
        // entry, so a malformed entry or a non-object is rejected here as before (the parse fails and
        // the record is dropped); what it does not do is allocate a tree of owned strings and numbers
        // per member, or serialise that tree a second time. On a replay that round trip was most of
        // the time per record.
        //
        // Member order is the entry's, as `preserve_order` gave it before. A value's text is written
        // as received rather than re-serialised, and for floats that is a correction, not only a
        // saving: serde_json's default `from_str` does not round floats correctly (its exact mode is
        // the `float_roundtrip` feature, not enabled here), so the old parse-then-print read the
        // emitter's shortest spelling of an f64 — `3.846153617588737e-10` — as the f64 one ULP
        // below it and printed that: `3.8461536175887366e-10`. About half the float fields in every
        // structured log (`p_estimate`, `period_local`, `period_s`, …) were off by one ULP from the
        // value the algorithm held. Passing the emitter's text through writes the value it
        // computed. (A hand-written entry with, say, `1e2` now also keeps `1e2` where it was
        // rewritten as `100.0`; the daemon emits no such entries.)
        let Ok(members) = parse_json_members(&entry_json) else {
            return;
        };

        // A record that carries no `seq` of its own takes the nearest enclosing span's, if that
        // span was remembered by `on_new_span`.
        let span_seq = if members.iter().any(|(key, _)| *key == "seq") {
            None
        } else {
            ctx.event_span(event).and_then(|span| {
                span.scope()
                    .find_map(|s| s.extensions().get::<SpanSeq>().map(|x| x.0))
            })
        };

        // The line is formatted into a buffer of its own, outside the lock, and written in one call
        // under it. The daemon has several threads writing one stream, so a line must reach the file
        // whole: holding the lock only for the `write_all` keeps lines from interleaving while
        // keeping the hold as short as before (the old code also built the whole line first). The
        // buffer is one allocation per line; the round trip it replaces made about twenty.
        // `Write` errors are dropped, as before: these streams are best-effort data, not a
        // transactional log.
        let mut line = Vec::with_capacity(entry_json.len() + 128);
        if write_json_line(
            &mut line,
            self.timestamp_source.now(),
            &members,
            span_seq,
            self.version,
        )
        .is_err()
        {
            return;
        }
        if let Ok(mut guard) = self.writer.lock()
            && let Some(ref mut w) = *guard
        {
            let _ = w.write_all(&line);
        }
    }
}

/// Parse a JSON object into its members, each value left as the text it was written as.
///
/// `serde_json` deserialises a JSON object into a `Vec` of pairs only through its map visitor, which
/// is what this does by hand: the key as a borrowed `&str`, the value as a borrowed [`RawValue`]. A
/// non-object (an array, a bare string) is an error, so a record that is not an object is dropped by
/// the caller exactly as the previous `Map<String, Value>` parse dropped it.
fn parse_json_members(entry_json: &str) -> Result<Vec<(&str, &RawValue)>, serde_json::Error> {
    use serde::de::{Deserializer, MapAccess, Visitor};

    struct Members;
    impl<'de> Visitor<'de> for Members {
        type Value = Vec<(&'de str, &'de RawValue)>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a JSON object")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut members = Vec::with_capacity(map.size_hint().unwrap_or(8));
            while let Some(member) = map.next_entry::<&'de str, &'de RawValue>()? {
                members.push(member);
            }
            Ok(members)
        }
    }
    // serde's model: a Deserializer walks the input and reports what it finds to a Visitor. The
    // serde_json Deserializer is asked for one map (`deserialize_map`), and the visitor accepts
    // only that (`visit_map`), so an array or a bare value fails as "expected a JSON object". Each
    // entry is read as (&str key, &RawValue value): both borrow from the input, the RawValue being
    // the span of text of the value, which serde_json has checked is well-formed JSON but has not
    // turned into numbers or strings.
    let mut de = serde_json::Deserializer::from_str(entry_json);
    let members = de.deserialize_map(Members)?;
    de.end()?; // nothing may follow the object: `{"a":1} x` is an error, as it is for `from_str`
    Ok(members)
}

/// Write one log line: `{"timestamp":…,"event":…,"seq":…,<members>,"version":…}` and a newline.
///
/// Field order, from the outside in: `timestamp` first when the stream has one; then the entry's
/// members in their own order, with `seq` inserted right after the `event` member when the span
/// supplies it; `version` last. Keys are written with `serde_json` so that they are escaped as JSON
/// strings; values are written as their raw text (see `on_event`).
fn write_json_line<W: Write>(
    w: &mut W,
    timestamp: Option<DateTime<Utc>>,
    members: &[(&str, &RawValue)],
    span_seq: Option<u64>,
    version: &str,
) -> std::io::Result<()> {
    // The line is assembled in one pass, in this order:
    //   {"timestamp":…, <the entry's members, with "seq":N inserted after "event">, "version":…}\n
    // `sep` writes the comma between fields: nothing before the first, a comma before each next.
    w.write_all(b"{")?;
    let mut first = true;
    let mut sep = |w: &mut W| -> std::io::Result<()> {
        if first {
            first = false;
            Ok(())
        } else {
            w.write_all(b",")
        }
    };
    if let Some(now) = timestamp {
        sep(w)?;
        write!(w, "\"timestamp\":\"{}\"", format_log_timestamp(now))?;
    }
    for (key, value) in members {
        sep(w)?;
        // The key goes through serde_json so it is quoted and escaped as a JSON string; the value
        // is the text the emitter wrote, copied as is (see `on_event`).
        serde_json::to_writer(&mut *w, key)?;
        w.write_all(b":")?;
        w.write_all(value.get().as_bytes())?;
        if *key == "event"
            && let Some(seq) = span_seq
        {
            write!(w, ",\"seq\":{seq}")?;
        }
    }
    sep(w)?;
    w.write_all(b"\"version\":")?;
    serde_json::to_writer(&mut *w, version)?;
    w.write_all(b"}\n")
}

/// A span's `seq` field, kept in the span's extensions for the records emitted inside it.
struct SpanSeq(u64);

/// Visitor that extracts a `seq` field from a span's attributes.
#[derive(Default)]
struct SeqVisitor {
    seq: Option<u64>,
}

impl Visit for SeqVisitor {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "seq" {
            self.seq = Some(value);
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn fmt::Debug) {}
}

/// Visitor that extracts the `entry` field from a tracing event.
#[derive(Default)]
struct EntryVisitor {
    entry: Option<String>,
}

impl Visit for EntryVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "entry" {
            self.entry = Some(value.to_string());
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn fmt::Debug) {
        // Ignored — we only care about the `entry` string field
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, Value};
    use std::io::Read;
    use tempfile::TempDir;
    use tracing_subscriber::layer::SubscriberExt;

    const TEST_TARGET: &str = "test::structured";
    const TEST_VERSION: &str = "1.2.3";

    /// Helper: set up a layer writing to a temp file and run a closure with
    /// the subscriber active. Returns the file contents after the closure completes.
    fn with_layer(source: TimestampSource, f: impl FnOnce()) -> String {
        let tmp_dir = TempDir::new().unwrap();
        let (layer, handle) = StructuredLogLayer::new(
            tmp_dir.path(),
            "test.log",
            TEST_TARGET,
            TEST_VERSION,
            source,
        );

        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, f);
        handle.flush(); // buffered until flushed, as in production; see `LogHandle::flush`

        let mut contents = String::new();
        File::open(tmp_dir.path().join("test.log"))
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        contents
    }

    /// Parse the first JSON line from the output.
    fn parse_first_line(output: &str) -> Map<String, Value> {
        let line = output.lines().next().expect("expected at least one line");
        serde_json::from_str(line).expect("expected valid JSON")
    }

    #[test]
    fn writes_entry_with_correct_target() {
        let output = with_layer(TimestampSource::WallClock, || {
            let json = r#"{"event":"test_event","value":42}"#;
            tracing::info!(target: TEST_TARGET, entry = json);
        });

        let map = parse_first_line(&output);
        assert_eq!(map["event"], "test_event");
        assert_eq!(map["value"], 42);
    }

    #[test]
    fn ignores_events_with_wrong_target() {
        let output = with_layer(TimestampSource::WallClock, || {
            let json = r#"{"event":"test_event","value":42}"#;
            tracing::info!(target: "wrong::target", entry = json);
        });

        assert!(output.is_empty());
    }

    #[test]
    fn injects_timestamp() {
        let output = with_layer(TimestampSource::WallClock, || {
            let json = r#"{"event":"test_event"}"#;
            tracing::info!(target: TEST_TARGET, entry = json);
        });

        let map = parse_first_line(&output);
        let timestamp = map["timestamp"].as_str().unwrap();
        // Verify format: YYYY-MM-DDTHH:MM:SS.xxxxxxZ
        assert!(timestamp.ends_with('Z'));
        assert_eq!(timestamp.len(), 27);
        assert_eq!(&timestamp[4..5], "-");
        assert_eq!(&timestamp[10..11], "T");
    }

    #[test]
    fn fixed_timestamp_source_stamps_every_line_identically() {
        let output = with_layer(TimestampSource::Fixed, || {
            tracing::info!(target: TEST_TARGET, entry = r#"{"event":"a"}"#);
            tracing::info!(target: TEST_TARGET, entry = r#"{"event":"b"}"#);
        });

        let stamps: Vec<&str> = output
            .lines()
            .map(|l| {
                let map: Map<String, Value> = serde_json::from_str(l).unwrap();
                map["timestamp"].as_str().unwrap().to_owned().leak() as &str
            })
            .collect();
        assert_eq!(stamps, ["1970-01-01T00:00:00.000000Z"; 2]);
    }

    #[test]
    fn injects_version() {
        let output = with_layer(TimestampSource::WallClock, || {
            let json = r#"{"event":"test_event"}"#;
            tracing::info!(target: TEST_TARGET, entry = json);
        });

        let map = parse_first_line(&output);
        assert_eq!(map["version"], TEST_VERSION);
    }

    #[test]
    fn field_ordering_timestamp_event_fields_version() {
        let output = with_layer(TimestampSource::WallClock, || {
            let json = r#"{"event":"test_event","alpha":"first","beta":"second"}"#;
            tracing::info!(target: TEST_TARGET, entry = json);
        });

        let line = output.lines().next().unwrap();
        let map: Map<String, Value> = serde_json::from_str(line).unwrap();
        let keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();

        // Keys should be ordered: timestamp, event, alpha, beta, version
        assert_eq!(keys[0], "timestamp");
        assert_eq!(keys[1], "event");
        assert_eq!(keys.last().unwrap(), &"version");
    }

    #[test]
    fn preserves_nested_objects() {
        let output = with_layer(TimestampSource::WallClock, || {
            let json = r#"{"event":"test_event","clock":{"timestamp":"2026-07-16T14:33:05.000000Z","clock_error_bound_ns":108000}}"#;
            tracing::info!(target: TEST_TARGET, entry = json);
        });

        let map = parse_first_line(&output);
        let clock = map["clock"].as_object().unwrap();
        assert_eq!(clock["timestamp"], "2026-07-16T14:33:05.000000Z");
        assert_eq!(clock["clock_error_bound_ns"], 108000);
    }

    #[test]
    fn ignores_event_without_entry_field() {
        let output = with_layer(TimestampSource::WallClock, || {
            tracing::info!(target: TEST_TARGET, "a message without entry field");
        });

        assert!(output.is_empty());
    }

    #[test]
    fn ignores_malformed_json_in_entry() {
        let output = with_layer(TimestampSource::WallClock, || {
            tracing::info!(target: TEST_TARGET, entry = "not valid json {{{");
        });

        assert!(output.is_empty());
    }

    #[test]
    fn writes_multiple_entries() {
        let output = with_layer(TimestampSource::WallClock, || {
            let json1 = r#"{"event":"first"}"#;
            let json2 = r#"{"event":"second"}"#;
            tracing::info!(target: TEST_TARGET, entry = json1);
            tracing::info!(target: TEST_TARGET, entry = json2);
        });

        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);

        let map1: Map<String, Value> = serde_json::from_str(lines[0]).unwrap();
        let map2: Map<String, Value> = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(map1["event"], "first");
        assert_eq!(map2["event"], "second");
    }
    /// A record emitted inside a span on this layer's target, whose `seq` field it lacks, is
    /// stamped with the span's `seq` right after `event`. A span on another target is ignored.
    #[test]
    fn seq_is_stamped_from_a_span_on_the_layers_target() {
        let output = with_layer(TimestampSource::Fixed, || {
            let _own = tracing::trace_span!(target: TEST_TARGET, "feed", seq = 7_u64).entered();
            tracing::info!(target: TEST_TARGET, entry = r#"{"event":"x","x":{}}"#);
            let _other = tracing::trace_span!(target: "elsewhere", "feed", seq = 9_u64).entered();
            tracing::info!(target: TEST_TARGET, entry = r#"{"event":"y","y":{}}"#);
            tracing::info!(target: TEST_TARGET, entry = r#"{"event":"z","seq":3,"z":{}}"#);
        });
        let lines: Vec<&str> = output.lines().collect();
        assert!(
            lines[0].contains(r#""event":"x","seq":7,"x""#),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].contains(r#""event":"y","seq":7,"y""#),
            "{}",
            lines[1]
        ); // nearest own span
        assert!(
            lines[2].contains(r#""event":"z","seq":3,"z""#),
            "{}",
            lines[2]
        ); // its own seq wins
    }
    /// Rotation: a line written before `reopen` is flushed into the file that was renamed away,
    /// and a line written after goes to the new file at the original path. Nothing stays in a buffer.
    #[test]
    fn reopen_flushes_the_old_file_and_starts_the_new_one() {
        let tmp_dir = TempDir::new().unwrap();
        let path = tmp_dir.path().join("test.log");
        let rotated = tmp_dir.path().join("test.log.1");
        let (layer, handle) = StructuredLogLayer::new(
            tmp_dir.path(),
            "test.log",
            TEST_TARGET,
            TEST_VERSION,
            TimestampSource::None,
        );
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: TEST_TARGET, entry = r#"{"event":"before"}"#);
            // The line is in the buffer, not yet in the file.
            assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
            std::fs::rename(&path, &rotated).unwrap(); // what logrotate does
            handle.reopen(); // what SIGHUP triggers
            tracing::info!(target: TEST_TARGET, entry = r#"{"event":"after"}"#);
        });
        handle.flush();
        let read = |p: &Path| std::fs::read_to_string(p).unwrap();
        assert_eq!(
            read(&rotated).trim_end(),
            r#"{"event":"before","version":"1.2.3"}"#
        );
        assert_eq!(
            read(&path).trim_end(),
            r#"{"event":"after","version":"1.2.3"}"#
        );
    }

    /// Values are written as the text they arrived as: a string holding an escaped quote and a brace,
    /// a nested object, a number, a null, all pass through byte for byte, with the keys in their order.
    #[test]
    fn values_pass_through_as_written() {
        let entry = r#"{"event":"x","s":"a \"quoted\" } brace","n":{"a":[1,2.5,null]},"z":-3}"#;
        let output = with_layer(TimestampSource::None, || {
            tracing::info!(target: TEST_TARGET, entry = entry);
        });
        assert_eq!(
            output.trim_end(),
            r#"{"event":"x","s":"a \"quoted\" } brace","n":{"a":[1,2.5,null]},"z":-3,"version":"1.2.3"}"#
        );
    }

    /// A float is written as the emitter spelled it. serde_json's default `from_str` is not correctly
    /// rounded: `3.846153617588737e-10` (the shortest spelling of f64 bits 0x3dfa6e399992ee38) parses
    /// to the neighbour 0x3dfa6e399992ee37, which prints as `3.8461536175887366e-10`. The previous
    /// parse-then-print path wrote that neighbour; this pins that the spelling now passes through.
    #[test]
    fn floats_keep_the_emitters_spelling_not_a_neighbour() {
        let emitted = 3.846153617588737e-10_f64;
        let spelled = serde_json::to_string(&emitted).unwrap();
        assert_eq!(spelled, "3.846153617588737e-10");
        // The defect this guards against, shown rather than assumed: the round trip lands one ULP off.
        let reparsed: f64 = serde_json::from_str(&spelled).unwrap();
        assert_ne!(
            reparsed.to_bits(),
            emitted.to_bits(),
            "serde_json's parse has become exact; this test and the comment in on_event can go"
        );
        assert_eq!(
            serde_json::to_string(&reparsed).unwrap(),
            "3.8461536175887366e-10"
        );

        let output = with_layer(TimestampSource::None, || {
            tracing::info!(target: TEST_TARGET, entry = r#"{"event":"x","x":{"p":3.846153617588737e-10}}"#);
        });
        assert_eq!(
            output.trim_end(),
            r#"{"event":"x","x":{"p":3.846153617588737e-10},"version":"1.2.3"}"#
        );
    }

    /// `TimestampSource::None` writes no `timestamp` field at all; the line starts at `event`.
    #[test]
    fn no_timestamp_source_omits_the_field() {
        let output = with_layer(TimestampSource::None, || {
            tracing::info!(target: TEST_TARGET, entry = r#"{"event":"x","x":{}}"#);
        });
        assert_eq!(
            output.trim_end(),
            r#"{"event":"x","x":{},"version":"1.2.3"}"#
        );
    }
}
