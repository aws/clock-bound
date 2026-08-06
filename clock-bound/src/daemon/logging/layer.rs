//! Custom tracing [`Layer`] for structured log files.
//!
//! Provides [`StructuredLogLayer`], which writes newline-delimited JSON to a file
//! with automatic injection of `timestamp` and `version` fields.

use std::{
    fmt,
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
    sync::Mutex,
};

use chrono::Utc;
use serde_json::{Map, Value};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{layer::Context, registry::LookupSpan};

/// A tracing [`Layer`] that writes pre-serialized JSON entries as newline-delimited
/// JSON to a file, injecting `timestamp` and `version` fields automatically.
///
/// Events are filtered by tracing target prefix. The layer expects an `entry` field
/// containing a JSON-serialized string.
///
/// This layer/target is not intended to be called into directly by the main app,
/// but via strongly-typed helper libraries.
pub struct StructuredLogLayer {
    writer: Mutex<File>,
    target_prefix: &'static str,
    version: &'static str,
}

impl StructuredLogLayer {
    /// Create a new `StructuredLogLayer` that writes to the specified file.
    ///
    /// # Arguments
    /// - `log_directory` - Directory where the log file will be created
    /// - `filename` - Name of the log file (e.g., `"synchronization.log"`)
    /// - `target_prefix` - Tracing target prefix to filter events (e.g., `"clock_bound::synchronization"`)
    /// - `version` - Version string to inject into every log entry
    ///
    /// # Panics
    /// Panics if the log file cannot be opened or created at `log_directory/filename`.
    pub fn new(
        log_directory: impl AsRef<Path>,
        filename: &str,
        target_prefix: &'static str,
        version: &'static str,
    ) -> Self {
        let path = log_directory.as_ref().join(filename);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("Failed to open log file {}: {}", path.display(), e));

        Self {
            writer: Mutex::new(file),
            target_prefix,
            version,
        }
    }
}

impl<S> tracing_subscriber::Layer<S> for StructuredLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if !event.metadata().target().starts_with(self.target_prefix) {
            return;
        }

        let mut visitor = EntryVisitor::default();
        event.record(&mut visitor);

        let Some(entry_json) = visitor.entry else {
            return;
        };

        // Deserialize the entry JSON into a map so we can inject fields
        //
        // This deserialization -> reserialization is the price we pay for the convenience
        // of continuing to use `tracing` for these structured logs.
        // May be worth revisiting in the future.
        let Ok(entry_map) = serde_json::from_str::<Map<String, Value>>(&entry_json) else {
            return;
        };

        // Build output with desired field ordering: timestamp, event fields, version
        let mut output = Map::new();
        output.insert(
            "timestamp".into(),
            Value::String(Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()),
        );
        output.extend(entry_map);
        output.insert("version".into(), Value::String(self.version.into()));

        // Write as a single JSON line
        if let Ok(line) = serde_json::to_string(&output)
            && let Ok(mut w) = self.writer.lock()
        {
            let _ = writeln!(w, "{line}");
        }
    }
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
    use std::io::Read;
    use tempfile::TempDir;
    use tracing_subscriber::layer::SubscriberExt;

    const TEST_TARGET: &str = "test::structured";
    const TEST_VERSION: &str = "1.2.3";

    /// Helper: set up a layer writing to a temp file and run a closure with
    /// the subscriber active. Returns the file contents after the closure completes.
    fn with_layer(f: impl FnOnce()) -> String {
        let tmp_dir = TempDir::new().unwrap();
        let layer = StructuredLogLayer::new(tmp_dir.path(), "test.log", TEST_TARGET, TEST_VERSION);

        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, f);

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
        let output = with_layer(|| {
            let json = r#"{"event":"test_event","value":42}"#;
            tracing::info!(target: TEST_TARGET, entry = json);
        });

        let map = parse_first_line(&output);
        assert_eq!(map["event"], "test_event");
        assert_eq!(map["value"], 42);
    }

    #[test]
    fn ignores_events_with_wrong_target() {
        let output = with_layer(|| {
            let json = r#"{"event":"test_event","value":42}"#;
            tracing::info!(target: "wrong::target", entry = json);
        });

        assert!(output.is_empty());
    }

    #[test]
    fn injects_timestamp() {
        let output = with_layer(|| {
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
    fn injects_version() {
        let output = with_layer(|| {
            let json = r#"{"event":"test_event"}"#;
            tracing::info!(target: TEST_TARGET, entry = json);
        });

        let map = parse_first_line(&output);
        assert_eq!(map["version"], TEST_VERSION);
    }

    #[test]
    fn field_ordering_timestamp_event_fields_version() {
        let output = with_layer(|| {
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
        let output = with_layer(|| {
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
        let output = with_layer(|| {
            tracing::info!(target: TEST_TARGET, "a message without entry field");
        });

        assert!(output.is_empty());
    }

    #[test]
    fn ignores_malformed_json_in_entry() {
        let output = with_layer(|| {
            tracing::info!(target: TEST_TARGET, entry = "not valid json {{{");
        });

        assert!(output.is_empty());
    }

    #[test]
    fn writes_multiple_entries() {
        let output = with_layer(|| {
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
}
