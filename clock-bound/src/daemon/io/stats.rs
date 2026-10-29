//! IO stats data shared between a source's IO task and the daemon
//!
//! A source's IO task counts the requests it makes and the responses it gets on an [`IOStats`]
//! as they happen. The daemon holds a clone of the same handle and reads an [`IOStatsSnapshot`]
//! when it writes a snapshot to `sources.log`.

use std::sync::{Arc, Mutex};

/// A consistent copy of a source's [`IOStats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IOStatsSnapshot {
    /// Requests sent, over the lifetime of the source.
    pub requests: u64,
    /// Responses received, over the lifetime of the source.
    pub responses: u64,
}

/// Request and response counters for one source.
///
/// Cloning gives another handle to the same counters, so the IO task and the daemon can share
/// them.
#[derive(Debug, Clone)]
pub struct IOStats(Arc<Mutex<IOStatsSnapshot>>);

impl IOStats {
    /// Counters for a source that has not made a request yet.
    pub fn empty() -> Self {
        Self(Arc::new(Mutex::new(IOStatsSnapshot {
            requests: 0,
            responses: 0,
        })))
    }

    /// Count a request that has been sent.
    pub fn record_request(&self) {
        #[expect(clippy::missing_panics_doc, reason = "not handling mutex poison")]
        let mut stats = self.0.lock().unwrap();
        stats.requests += 1;
    }

    /// Count a response that has been received.
    pub fn record_response(&self) {
        #[expect(clippy::missing_panics_doc, reason = "not handling mutex poison")]
        let mut stats = self.0.lock().unwrap();
        stats.responses += 1;
    }

    /// A consistent copy of the current counters.
    pub fn snapshot(&self) -> IOStatsSnapshot {
        #[expect(clippy::missing_panics_doc, reason = "not handling mutex poison")]
        let stats = self.0.lock().unwrap();
        *stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stats_have_no_requests_or_responses() {
        let snapshot = IOStats::empty().snapshot();
        assert_eq!(snapshot.requests, 0);
        assert_eq!(snapshot.responses, 0);
    }

    #[test]
    fn requests_and_responses_are_counted_separately() {
        let stats = IOStats::empty();
        stats.record_request();
        stats.record_request();
        stats.record_request();
        stats.record_response();

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.requests, 3);
        assert_eq!(snapshot.responses, 1);
    }

    #[test]
    fn clones_share_the_same_counters() {
        let stats = IOStats::empty();
        let task_handle = stats.clone();

        task_handle.record_request();
        task_handle.record_response();

        assert_eq!(stats.snapshot(), task_handle.snapshot());
        assert_eq!(stats.snapshot().requests, 1);
        assert_eq!(stats.snapshot().responses, 1);
    }
}
