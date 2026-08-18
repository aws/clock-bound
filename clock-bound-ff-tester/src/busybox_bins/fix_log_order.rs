//! Fix the line ordering of a log pulled from a delivery pipeline (e.g. `CloudWatch`)
//! and strip delivery prefixes, producing a replay-ready NDJSON file.
//!
//! Logs pulled via `aws logs tail` differ from the file the daemon wrote in two
//! ways: each line is prefixed with delivery metadata (ingestion timestamp,
//! instance id), and lines sharing the same ingestion timestamp may be delivered
//! out of order. This tool undoes both, so that a replay sees exactly the sequence
//! the daemon emitted. The replay tooling rejects prefixed lines, enforcing that
//! pulled logs are fixed once (per dataset) before they can be replayed.
//!
//! # Reordering strategy
//!
//! Delivery scrambling is strictly local to a batch of lines sharing the same
//! ingestion timestamp (verified on real captures). Within a batch, two
//! independent physical signals define the original order: the inner timestamp
//! and the counter at I/O completion (see [`crate::logs::order`]). A batch is
//! reordered only when both signals agree on an order that differs from the
//! delivered one. If the signals disagree, the anomaly is the daemon's own
//! dynamics (clock step, async I/O reordering) and the delivered order is
//! preserved: reordering would corrupt what the algorithm actually experienced.

use std::{
    fs::File,
    io::{BufRead, BufWriter, Write},
    path::PathBuf,
};

use clap::Parser;
use flate2::{Compression, write::GzEncoder};
use tracing::Level;
use tracing_subscriber::EnvFilter;

use crate::logs::{
    open_log,
    order::{OrderAnomalyKind, OrderTracker, OrderingSignals},
};

#[derive(Parser, Clone, Debug)]
/// Fix the line ordering of a delivery-scrambled log and strip delivery
/// prefixes, producing a replay-ready NDJSON file
pub struct Cli {
    /// The logfile to fix (NDJSON, optionally gzip compressed, optionally with
    /// delivery prefixes as produced by `aws logs tail`)
    #[arg(short, long)]
    logfile: PathBuf,

    /// The fixed output file. Written gzip-compressed if the name ends in `.gz`.
    /// Required unless running with --check.
    #[arg(short, long, required_unless_present = "check")]
    output: Option<PathBuf>,

    /// Analyze and report only: do not write an output file
    #[arg(long)]
    check: bool,

    /// Overwrite the output file if it already exists
    #[arg(long)]
    force: bool,
}

pub fn main(args: Cli) -> anyhow::Result<()> {
    // try_init: tolerate an already-installed subscriber (e.g. when invoked
    // from tests or another in-process caller)
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(Level::INFO.into())
                .from_env_lossy(),
        )
        .try_init();

    let mut writer = match (&args.output, args.check) {
        (Some(path), false) => {
            if path.exists() && !args.force {
                anyhow::bail!(
                    "Output file {} already exists (use --force to overwrite)",
                    path.display()
                );
            }
            Some(make_writer(path)?)
        }
        _ => None, // --check: analyze and report only
    };

    let mut stats = FixStats::default();
    let mut tracker = OrderTracker::new();

    // Current batch of consecutive lines sharing the same delivery prefix key
    let mut batch_key: Option<String> = None;
    let mut batch: Vec<Line> = Vec::new();

    for (index, line) in BufRead::lines(open_log(&args.logfile)?).enumerate() {
        let line = line?;
        stats.lines_in += 1;

        // Strip the delivery prefix: the payload starts at the first '{'
        let Some(start) = line.find('{') else {
            tracing::warn!(line = index, "no JSON payload on line, dropping: {line}");
            stats.lines_dropped += 1;
            continue;
        };
        let key = (start > 0).then(|| {
            line[..start]
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string()
        });
        let payload = line[start..].to_string();
        let signals = OrderingSignals::extract(&payload);

        // A change of delivery key closes the current batch
        if key != batch_key || key.is_none() {
            flush_batch(&mut batch, &mut tracker, &mut stats, &mut writer)?;
            batch_key = key;
        }
        batch.push(Line {
            index,
            payload,
            signals,
        });
    }
    flush_batch(&mut batch, &mut tracker, &mut stats, &mut writer)?;

    if let Some(w) = &mut writer {
        w.flush()?;
    }

    let summary = tracker.summary();
    tracing::info!(
        lines_in = stats.lines_in,
        lines_out = stats.lines_in - stats.lines_dropped,
        lines_dropped = stats.lines_dropped,
        scrambles_fixed = stats.scrambles_fixed,
        conflicts_preserved = stats.conflicts_preserved,
        %summary,
        "fix-log-order complete"
    );
    Ok(())
}

/// Create the output writer, gzip-compressing when the file name ends in `.gz`.
fn make_writer(path: &std::path::Path) -> anyhow::Result<Box<dyn Write>> {
    let file = File::create(path)?;
    Ok(if path.extension().is_some_and(|e| e == "gz") {
        Box::new(BufWriter::new(GzEncoder::new(file, Compression::default())))
    } else {
        Box::new(BufWriter::new(file))
    })
}

struct Line {
    /// 0-based line index in the input file, for diagnostics
    index: usize,
    payload: String,
    signals: OrderingSignals,
}

#[derive(Debug, Default)]
struct FixStats {
    lines_in: usize,
    lines_dropped: usize,
    scrambles_fixed: usize,
    conflicts_preserved: usize,
}

/// Emit the current batch in corrected order, then clear it.
fn flush_batch(
    batch: &mut Vec<Line>,
    tracker: &mut OrderTracker,
    stats: &mut FixStats,
    writer: &mut Option<Box<dyn Write>>,
) -> anyhow::Result<()> {
    if batch.is_empty() {
        return Ok(());
    }

    let signals: Vec<&OrderingSignals> = batch.iter().map(|l| &l.signals).collect();
    let (order, outcome) = batch_order(&signals);
    match outcome {
        BatchOutcome::Fixed => stats.scrambles_fixed += 1,
        BatchOutcome::PreservedConflict => {
            stats.conflicts_preserved += 1;
            tracing::warn!(
                "batch of {} lines has conflicting ordering signals, preserving delivered order",
                batch.len()
            );
        }
        BatchOutcome::Identity => {}
    }

    for &i in &order {
        let line = &batch[i];
        // Classify and report anomalies on the *output* stream, with input file
        // line numbers: after fixing, whatever remains is the daemon's own
        // dynamics (clock steps, async I/O reordering), reported with magnitudes.
        // A remaining delivery scramble would mean cross-batch displacement,
        // which the fix cannot repair.
        if let Some(anomaly) = tracker.observe(line.index, &line.signals) {
            match anomaly.kind() {
                OrderAnomalyKind::DeliveryScramble => {
                    tracing::warn!("unfixable (cross-batch) {anomaly}");
                }
                OrderAnomalyKind::ClockStep | OrderAnomalyKind::AsyncReorder => {
                    tracing::info!("{anomaly}");
                }
            }
        }
        if let Some(w) = writer.as_deref_mut() {
            writeln!(w, "{}", line.payload)?;
        }
    }
    batch.clear();
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchOutcome {
    /// Delivered order kept: nothing out of order
    Identity,
    /// Both ordering signals agreed on a different order: reordered
    Fixed,
    /// Ordering signals disagree with each other: delivered order kept
    PreservedConflict,
}

/// Decide the output order of a batch of lines sharing one delivery key.
///
/// A batch is reordered only when every line carries both ordering signals and
/// the timestamp order and counter order agree with each other while differing
/// from the delivered order. Everything else preserves the delivered order: a
/// line missing a signal (e.g. init/disruption), or signals that disagree with
/// each other (the daemon's own dynamics — clock step, async I/O reordering).
fn batch_order(signals: &[&OrderingSignals]) -> (Vec<usize>, BatchOutcome) {
    let delivered: Vec<usize> = (0..signals.len()).collect();

    // Conservative: any line missing a signal makes the whole batch unsortable
    let all_sortable = signals
        .iter()
        .all(|s| s.timestamp.is_some() && s.counter_post.is_some());
    if signals.len() < 2 || !all_sortable {
        return (delivered, BatchOutcome::Identity);
    }

    let mut by_timestamp = delivered.clone();
    by_timestamp.sort_by_key(|&i| signals[i].timestamp.as_deref());
    let mut by_counter = delivered.clone();
    by_counter.sort_by_key(|&i| signals[i].counter_post);

    if by_timestamp == by_counter && by_timestamp != delivered {
        (by_timestamp, BatchOutcome::Fixed)
    } else if by_timestamp != delivered && by_counter != delivered && by_timestamp != by_counter {
        // Both signals see disorder but disagree on the repair: ambiguous
        (delivered, BatchOutcome::PreservedConflict)
    } else {
        // Disorder on one signal only (clock step or async I/O reordering) is
        // the daemon's own dynamics: preserve, the OrderTracker classifies and
        // reports it on the output stream.
        (delivered, BatchOutcome::Identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(ts: &str, post: Option<u64>) -> OrderingSignals {
        OrderingSignals {
            timestamp: Some(format!("2026-01-01T00:00:{ts}Z")),
            counter_post: post,
        }
    }

    #[test]
    fn batch_in_order_is_identity() {
        let s = [sig("01.0", Some(100)), sig("02.0", Some(200))];
        let refs: Vec<_> = s.iter().collect();
        let (order, outcome) = batch_order(&refs);
        assert_eq!(order, vec![0, 1]);
        assert_eq!(outcome, BatchOutcome::Identity);
    }

    #[test]
    fn scramble_is_fixed_when_signals_agree() {
        // Delivered 2nd-then-1st; both signals say swap back
        let s = [sig("02.0", Some(200)), sig("01.0", Some(100))];
        let refs: Vec<_> = s.iter().collect();
        let (order, outcome) = batch_order(&refs);
        assert_eq!(order, vec![1, 0]);
        assert_eq!(outcome, BatchOutcome::Fixed);
    }

    #[test]
    fn clock_step_is_preserved() {
        // Timestamp regresses but counters ascend: real daemon behavior
        let s = [sig("02.0", Some(100)), sig("01.0", Some(200))];
        let refs: Vec<_> = s.iter().collect();
        let (order, outcome) = batch_order(&refs);
        assert_eq!(order, vec![0, 1]);
        assert_eq!(outcome, BatchOutcome::Identity);
    }

    #[test]
    fn async_reorder_is_preserved() {
        // Counter regresses but timestamps ascend: real daemon behavior
        let s = [sig("01.0", Some(200)), sig("02.0", Some(100))];
        let refs: Vec<_> = s.iter().collect();
        let (order, outcome) = batch_order(&refs);
        assert_eq!(order, vec![0, 1]);
        assert_eq!(outcome, BatchOutcome::Identity);
    }

    #[test]
    fn counterless_line_is_a_barrier() {
        // A batch containing a counter-less line (e.g. init) is preserved whole
        let barrier = OrderingSignals {
            timestamp: Some("2026-01-01T00:00:01.5Z".to_string()),
            ..Default::default()
        };
        let s = [sig("02.0", Some(200)), barrier, sig("01.0", Some(100))];
        let refs: Vec<_> = s.iter().collect();
        let (order, _) = batch_order(&refs);
        assert_eq!(order, vec![0, 1, 2]);
    }

    #[test]
    fn conflicting_signals_are_preserved_and_flagged() {
        // Three lines where ts and counter each see disorder but disagree on
        // the repair
        let s = [
            sig("03.0", Some(200)),
            sig("01.0", Some(300)),
            sig("02.0", Some(100)),
        ];
        let refs: Vec<_> = s.iter().collect();
        let (order, outcome) = batch_order(&refs);
        assert_eq!(order, vec![0, 1, 2]);
        assert_eq!(outcome, BatchOutcome::PreservedConflict);
    }
}
