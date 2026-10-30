//! IO stats data shared between a source's IO task and the daemon
//!
//! A source's IO task counts the requests it makes and the responses it gets on an [`IOStats`]
//! as they happen, and remembers its most recent response. The daemon holds a clone of the same
//! handle and reads an [`IOStatsSnapshot`] when it writes a snapshot to `sources.log`.

use std::sync::{Arc, Mutex};

use crate::daemon::event;
use crate::daemon::io::ntp::{NtpIoError, UnusableNtpResponse};
use crate::daemon::logging::sources::SourceStatus;
use crate::daemon::time::{Duration, Instant, TscCount};

/// How recent a source's last response must be for it to count towards the source's status.
pub const RESPONSE_VALID_AGE: Duration = Duration::from_secs(60);

/// How a source answered a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseKind {
    /// The source answered, and it is synchronized.
    Synchronized,
    /// The source answered that it is unsynchronized.
    Unsynchronized,
    /// The response was unusable for any other reason.
    Other,
}

impl From<&UnusableNtpResponse> for ResponseKind {
    fn from(source: &UnusableNtpResponse) -> Self {
        match source {
            UnusableNtpResponse::Unsynchronized => Self::Unsynchronized,
            UnusableNtpResponse::InvalidStratum(_)
            | UnusableNtpResponse::KissODeath(_)
            | UnusableNtpResponse::BadTimestamps => Self::Other,
        }
    }
}

/// The most recent response from a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastResponse {
    /// TSC reading taken when the response was received.
    pub at: TscCount,
    pub kind: ResponseKind,
}

/// The outcome of a sample was not a response from the source.
#[derive(Debug)]
pub struct NoResponse;

impl TryFrom<&Result<event::Ntp, NtpIoError>> for LastResponse {
    type Error = NoResponse;

    /// The response the outcome of an NTP sample amounts to, if it was one.
    ///
    /// Any reply that matched our request is a response, whether or not it turned out to be
    /// usable. Failing to get a reply, because of a socket error or a timeout, is not.
    #[allow(clippy::cast_possible_wrap)]
    fn try_from(result: &Result<event::Ntp, NtpIoError>) -> Result<Self, Self::Error> {
        let (at, kind) = match result {
            Ok(event) => (event.counter_post(), ResponseKind::Synchronized),
            Err(NtpIoError::UnusableNtpResponse {
                source,
                received_at,
            }) => (
                TscCount::new(*received_at as i64),
                ResponseKind::from(source),
            ),
            // The server answered fine; it is our own TSC readings that are out of order.
            Err(NtpIoError::TscOrder { post, .. }) => {
                (TscCount::new(*post as i64), ResponseKind::Synchronized)
            }
            Err(NtpIoError::SampleIo(_) | NtpIoError::Bind(_) | NtpIoError::Timeout(_)) => {
                return Err(NoResponse);
            }
        };
        Ok(LastResponse { at, kind })
    }
}

/// A consistent copy of a source's [`IOStats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IOStatsSnapshot {
    /// Requests sent, over the lifetime of the source.
    pub requests: u64,
    /// Responses received, over the lifetime of the source.
    pub responses: u64,
    /// The most recent response, if any since the source started or the last disruption.
    pub last_response: Option<LastResponse>,
}

impl IOStatsSnapshot {
    /// The status of the source from its last response.
    ///
    /// The source is `Synchronized` or `FreeRunning` according to its last response if that was
    /// received at or after `oldest_response`, with `time_at` converting the TSC of the response
    /// to a time. It is `Unknown` if there is no such response, or if the last response was
    /// unusable for another reason.
    pub fn status(
        &self,
        oldest_response: Instant,
        time_at: impl Fn(TscCount) -> Instant,
    ) -> SourceStatus {
        let Some(last_response) = self.last_response else {
            return SourceStatus::Unknown;
        };
        if time_at(last_response.at) < oldest_response {
            return SourceStatus::Unknown;
        }
        match last_response.kind {
            ResponseKind::Synchronized => SourceStatus::Synchronized,
            ResponseKind::Unsynchronized => SourceStatus::FreeRunning,
            ResponseKind::Other => SourceStatus::Unknown,
        }
    }
}

/// Request and response stats for one source.
///
/// Cloning gives another handle to the same stats, so the IO task and the daemon can share them.
#[derive(Debug, Clone, Default)]
pub struct IOStats(Arc<Mutex<IOStatsSnapshot>>);

impl IOStats {
    /// Stats for a source that has not made a request yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Count a request that has been sent.
    pub fn record_request(&self) {
        #[expect(clippy::missing_panics_doc, reason = "not handling mutex poison")]
        let mut stats = self.0.lock().unwrap();
        stats.requests += 1;
    }

    /// Count a response that has been received, which becomes the last response.
    pub fn record_response(&self, last_response: LastResponse) {
        #[expect(clippy::missing_panics_doc, reason = "not handling mutex poison")]
        let mut stats = self.0.lock().unwrap();
        stats.responses += 1;
        stats.last_response = Some(last_response);
    }

    /// Forget the last response during a disruption, as the TSC can jump. The counters keep
    /// running.
    pub fn handle_disruption(&self) {
        #[expect(clippy::missing_panics_doc, reason = "not handling mutex poison")]
        let mut stats = self.0.lock().unwrap();
        stats.last_response = None;
    }

    /// A consistent copy of the current stats.
    pub fn snapshot(&self) -> IOStatsSnapshot {
        #[expect(clippy::missing_panics_doc, reason = "not handling mutex poison")]
        let stats = self.0.lock().unwrap();
        *stats
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, ErrorKind};

    use rstest::rstest;

    use super::*;
    use crate::daemon::time::{Duration, Instant};

    fn ntp_event() -> event::Ntp {
        event::Ntp::builder()
            .counter_pre(TscCount::new(1_000))
            .counter_post(TscCount::new(2_000))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_secs(10),
                server_send_time: Instant::from_secs(11),
                root_delay: Duration::from_micros(100),
                root_dispersion: Duration::from_micros(50),
                stratum: event::Stratum::ONE,
            })
            .build()
            .expect("valid NTP event")
    }

    // A reply to our request is the last response, stamped with the receive TSC. Failing to get
    // a reply is none.
    #[rstest]
    #[case::usable(Ok(ntp_event()), Some((2_000, ResponseKind::Synchronized)))]
    #[case::unsynchronized(
        Err(NtpIoError::UnusableNtpResponse {
            source: UnusableNtpResponse::Unsynchronized,
            received_at: 42,
        }),
        Some((42, ResponseKind::Unsynchronized))
    )]
    #[case::kiss_o_death(
        Err(NtpIoError::UnusableNtpResponse {
            source: UnusableNtpResponse::KissODeath([0; 4]),
            received_at: 42,
        }),
        Some((42, ResponseKind::Other))
    )]
    #[case::bad_timestamps(
        Err(NtpIoError::UnusableNtpResponse {
            source: UnusableNtpResponse::BadTimestamps,
            received_at: 42,
        }),
        Some((42, ResponseKind::Other))
    )]
    #[case::invalid_stratum(
        Err(NtpIoError::UnusableNtpResponse {
            source: UnusableNtpResponse::InvalidStratum(event::TryFromU8Error),
            received_at: 42,
        }),
        Some((42, ResponseKind::Other))
    )]
    #[case::tsc_order(
        Err(NtpIoError::TscOrder { pre: 2, post: 1 }),
        Some((1, ResponseKind::Synchronized))
    )]
    #[case::sample_io(
        Err(NtpIoError::SampleIo(io::Error::from(ErrorKind::ConnectionRefused))),
        None
    )]
    #[case::bind(
        Err(NtpIoError::Bind(io::Error::from(ErrorKind::AddrNotAvailable))),
        None
    )]
    fn last_response_comes_from_any_reply_to_our_request(
        #[case] result: Result<event::Ntp, NtpIoError>,
        #[case] expected: Option<(i64, ResponseKind)>,
    ) {
        let expected = expected.map(|(at, kind)| LastResponse {
            at: TscCount::new(at),
            kind,
        });
        assert_eq!(LastResponse::try_from(&result).ok(), expected);
    }

    #[tokio::test]
    async fn timeout_is_not_a_last_response() {
        // `time::error::Elapsed` has a private constructor, so obtain one by
        // actually timing out a pending future.
        let elapsed = tokio::time::timeout(
            std::time::Duration::from_nanos(1),
            futures::future::pending::<()>(),
        )
        .await
        .unwrap_err();
        assert!(LastResponse::try_from(&Err(NtpIoError::Timeout(elapsed))).is_err());
    }

    fn response_at(at: i64, kind: ResponseKind) -> LastResponse {
        LastResponse {
            at: TscCount::new(at),
            kind,
        }
    }

    fn snapshot_with(last_response: Option<LastResponse>) -> IOStatsSnapshot {
        IOStatsSnapshot {
            requests: 0,
            responses: 0,
            last_response,
        }
    }

    // A response at `at` seconds, with one second per tick, against an oldest response at 100.
    #[rstest]
    #[case::synchronized(Some(ResponseKind::Synchronized), 150, SourceStatus::Synchronized)]
    #[case::unsynchronized(Some(ResponseKind::Unsynchronized), 150, SourceStatus::FreeRunning)]
    #[case::other(Some(ResponseKind::Other), 150, SourceStatus::Unknown)]
    #[case::at_the_oldest_response(
        Some(ResponseKind::Synchronized),
        100,
        SourceStatus::Synchronized
    )]
    #[case::stale_synchronized(Some(ResponseKind::Synchronized), 99, SourceStatus::Unknown)]
    #[case::stale_unsynchronized(Some(ResponseKind::Unsynchronized), 99, SourceStatus::Unknown)]
    #[case::no_response(None, 150, SourceStatus::Unknown)]
    fn status_follows_the_last_response_if_recent_enough(
        #[case] kind: Option<ResponseKind>,
        #[case] at: i64,
        #[case] expected: SourceStatus,
    ) {
        let snapshot = snapshot_with(kind.map(|kind| response_at(at, kind)));

        let status = snapshot.status(Instant::from_secs(100), |tsc| Instant::from_secs(tsc.get()));

        assert_eq!(status, expected);
    }

    #[test]
    fn new_stats_have_no_requests_or_responses() {
        let snapshot = IOStats::new().snapshot();
        assert_eq!(snapshot.requests, 0);
        assert_eq!(snapshot.responses, 0);
        assert_eq!(snapshot.last_response, None);
    }

    #[test]
    fn requests_and_responses_are_counted_separately() {
        let stats = IOStats::new();
        stats.record_request();
        stats.record_request();
        stats.record_request();
        stats.record_response(response_at(10, ResponseKind::Synchronized));

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.requests, 3);
        assert_eq!(snapshot.responses, 1);
    }

    #[test]
    fn the_newest_response_is_the_last_response() {
        let stats = IOStats::new();
        stats.record_response(response_at(10, ResponseKind::Synchronized));
        stats.record_response(response_at(20, ResponseKind::Unsynchronized));
        stats.record_response(response_at(30, ResponseKind::Other));

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.responses, 3);
        assert_eq!(
            snapshot.last_response,
            Some(response_at(30, ResponseKind::Other))
        );
    }

    #[test]
    fn disruption_clears_the_last_response_but_keeps_the_counters() {
        let stats = IOStats::new();
        stats.record_request();
        stats.record_response(response_at(10, ResponseKind::Synchronized));

        stats.handle_disruption();

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.requests, 1);
        assert_eq!(snapshot.responses, 1);
        assert_eq!(snapshot.last_response, None);
    }

    #[test]
    fn clones_share_the_same_stats() {
        let stats = IOStats::new();
        let task_handle = stats.clone();

        task_handle.record_request();
        task_handle.record_response(response_at(10, ResponseKind::Synchronized));

        assert_eq!(stats.snapshot(), task_handle.snapshot());
        assert_eq!(stats.snapshot().requests, 1);
        assert_eq!(stats.snapshot().responses, 1);
    }
}
