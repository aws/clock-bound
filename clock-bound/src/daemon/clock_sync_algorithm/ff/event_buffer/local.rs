//! Local event buffer
use std::{fmt::Debug, num::NonZeroUsize};

use crate::daemon::{
    clock_sync_algorithm::RingBuffer,
    event::TscRtt,
    time::{Duration, Instant, TscCount, TscDiff, tsc::Period},
};

/// A local buffer of events
///
/// This stores (as a maximum) the last 1024 seconds of clock sync events to stay within the
/// simple skew model (SKM) window.
///
/// The capacity of the ring buffer is configurable, as different reference clock sources may
/// have differing requirements.
///
/// ## Feeding data
/// The Local event buffer should have data fed in 2 different ways based on available information.
/// 1. If the TSC period is unknown, simply [`Self::feed`] in the data
/// 2. If the TSC period has an estimate, then `feed` and then [`Self::retain_high_quality_events`] to purge old values
///
/// ## Starvation
/// The Local event buffer currently has a simple rejection strategy. Have a multiplier. When attempting to feed
/// in a new event, if the `RTT(event) >= multiplier * min_rtt`, we reject the sample. We can increase complexity
/// of this with time and exploration of data samples.
///
/// However, this rule **can** cause starvation if the RTT is extremely noisy. That's why the threshold can be tuned
/// or even dynamically modified. Changes subject to experimentation.
#[derive(Debug, Clone)]
pub struct Local<T> {
    /// The inner storage
    inner: RingBuffer<T>,

    /// Rejection threshold when feeding events
    ///
    /// If a value is less than the `min_rtt * rtt_threshold_multiplier`,
    /// then we don't add it
    rtt_threshold_multiplier: usize,
}

impl<T> Local<T> {
    /// The simple skew model window length
    ///
    /// This is the period of time where a CPUs offset is generally approximated
    /// by a linear model
    pub const SKM_WINDOW: Duration = Duration::from_secs(1024);

    /// Creates a new ring buffer
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            inner: RingBuffer::new(capacity),
            rtt_threshold_multiplier: 5,
        }
    }

    /// Construct with a `rtt_threshold` multiplier
    pub fn with_rtt_threshold_multiplier(
        capacity: NonZeroUsize,
        rtt_threshold_multiplier: usize,
    ) -> Self {
        Self {
            inner: RingBuffer::new(capacity),
            rtt_threshold_multiplier,
        }
    }

    /// Returns an iterator of events from oldest to newest
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.inner.iter()
    }

    /// Returns true if empty
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl<T: TscRtt + Debug> Local<T> {
    /// Feed the ring buffer with a new event
    ///
    /// # Errors
    /// - the event is older than events already in the buffer
    /// - the event has too large of an rtt
    pub fn feed(&mut self, event: T) -> Result<(), FeedError<T>> {
        let rtt = event.rtt();
        let Some(min_rtt_event) = self.inner.min_rtt() else {
            // we empty
            self.inner.push(event);
            return Ok(());
        };

        let threshold = min_rtt_event.rtt() * self.rtt_threshold_multiplier;
        if rtt > threshold {
            return Err(FeedError::RttExceedsThreshold {
                max_diff: threshold,
                event,
            });
        }
        self.push_if_newer(event)
    }

    /// Adds a new event to the ring buffer
    ///
    /// Returns true if the event was added, false if it was rejected.
    /// The value could be rejected if it is older (via TSC checks) than previous values.
    fn push_if_newer(&mut self, event: T) -> Result<(), FeedError<T>> {
        if let Some(last) = self.inner.head()
            && event.tsc_post() <= last.tsc_post()
        {
            return Err(FeedError::Old {
                latest_tsc: last.tsc_post(),
                event,
            });
        }

        self.inner.push(event);
        Ok(())
    }
}

impl<T: TscRtt + Debug> Local<T> {
    /// Retain high quality events in the SKM window
    ///
    /// This will delete all values that either:
    /// - are older than the SKM window
    /// - has an RTT too large
    ///
    /// Requires a period measurement to convert the TSC values to time durations.
    ///
    /// # Starvation warning
    /// This will clear out anything older than the cutoff. This CAN leave the ring buffer EMPTY
    #[expect(clippy::missing_panics_doc, reason = "min_rtt has an element")]
    pub fn retain_high_quality_events(&mut self, period: Period, now_post_tsc: TscCount) {
        if self.is_empty() {
            return;
        }
        let max_rtt = self.inner.min_rtt().unwrap().rtt() * self.rtt_threshold_multiplier;

        let newest_time = now_post_tsc.uncorrected_time(period, Instant::UNIX_EPOCH);

        if newest_time < (Instant::EPOCH + Self::SKM_WINDOW) {
            self.inner.expunge_old_and_delayed(TscCount::EPOCH, max_rtt);
        } else {
            let cutoff = newest_time - Self::SKM_WINDOW;
            let cutoff_tsc = TscCount::from_uncorrected_time(cutoff, period, Instant::UNIX_EPOCH);

            self.inner.expunge_old_and_delayed(cutoff_tsc, max_rtt);
        }
    }
}

impl<T> AsRef<RingBuffer<T>> for Local<T> {
    fn as_ref(&self) -> &RingBuffer<T> {
        &self.inner
    }
}

#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum FeedError<T: std::fmt::Debug> {
    #[error(
        "event is older than latest event. Buffer's latest event TSC {latest_tsc:?}, event: {event:?}"
    )]
    Old { latest_tsc: TscCount, event: T },
    #[error("event has too large of an rtt. Threshold rtt {max_diff:?}, event: {event:?}")]
    RttExceedsThreshold { max_diff: TscDiff, event: T },
}

#[cfg(test)]
mod tests {
    use super::super::test_assets::TestEvent;
    use super::*;
    use std::num::NonZeroUsize;

    #[test]
    fn new_buffer() {
        let buffer: Local<TestEvent> = Local::new(NonZeroUsize::new(5).unwrap());
        assert!(buffer.as_ref().is_empty())
    }

    #[test]
    fn feed_empty_buffer() {
        let mut buffer: Local<TestEvent> = Local::new(NonZeroUsize::new(5).unwrap());
        let event = TestEvent::new(5, 10);

        assert!(buffer.feed(event).is_ok());
        assert_eq!(buffer.iter().count(), 1);
    }

    #[test]
    fn feed_rtt_threshold() {
        let mut buffer: Local<TestEvent> = Local::new(NonZeroUsize::new(5).unwrap());

        // First event with small RTT
        let event1 = TestEvent::new(10, 20); // rtt is 10
        assert!(buffer.feed(event1).is_ok());

        // Second event with RTT exceeding threshold (10 * 5 = 50)
        let event2 = TestEvent::new(30, 81);
        let err = buffer.feed(event2).unwrap_err();
        assert!(matches!(err, FeedError::RttExceedsThreshold { .. }));
    }

    #[test]
    fn push_stale() {
        let mut buffer: Local<TestEvent> = Local::new(NonZeroUsize::new(5).unwrap());

        // Add first event
        let event1 = TestEvent::new(100, 110);
        buffer.feed(event1).unwrap();

        // Try to add older event
        let event2 = TestEvent::new(50, 60);
        let err = buffer.feed(event2).unwrap_err();
        assert!(matches!(err, FeedError::Old { .. }));
    }

    #[test]
    fn push_newer() {
        let mut buffer: Local<TestEvent> = Local::new(NonZeroUsize::new(5).unwrap());

        // Add first event
        let event1 = TestEvent::new(100, 110);
        buffer.feed(event1).unwrap();

        // Add newer event
        let event3 = TestEvent::new(150, 180);
        buffer.feed(event3).unwrap();
    }

    #[test]
    fn retain_high_quality_events() {
        let mut buffer: Local<TestEvent> = Local::new(NonZeroUsize::new(5).unwrap());
        let period = Period::from_seconds(1e-9); // 1 ns per tick

        // Add events spanning more than 1024 seconds
        let events = vec![
            TestEvent::new(0, 10),
            TestEvent::new(500_000_000_000, 500_000_000_000 + 10), // 500 seconds
            TestEvent::new(1_500_000_000_000, 1_500_000_000_000 + 10), // 1500 seconds
        ];

        for event in events {
            buffer.feed(event).unwrap();
        }

        buffer.retain_high_quality_events(period, TscCount::new(1_500_000_000_000));

        // Should only retain events within last 1024 seconds
        for event in buffer.iter() {
            assert!(event.tsc_post().get() >= 500_000_000_000);
        }
    }

    #[test]
    fn buffer_capacity() {
        let mut buffer: Local<TestEvent> = Local::new(NonZeroUsize::new(3).unwrap());

        // Add more events than capacity
        for i in 0..5 {
            let event = TestEvent::new(i * 100, i * 100 + 10);
            assert!(buffer.feed(event).is_ok());
        }

        // Buffer should only contain last 3 events
        assert_eq!(buffer.iter().count(), 3);
        for (i, event) in buffer.iter().enumerate() {
            assert_eq!(event.tsc_pre().get(), ((i + 2) * 100) as i128);
        }
    }
}
