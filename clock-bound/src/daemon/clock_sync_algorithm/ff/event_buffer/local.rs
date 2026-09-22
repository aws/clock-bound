//! Local event buffer
use std::{fmt::Debug, num::NonZeroUsize};

use crate::daemon::{
    clock_sync_algorithm::RingBuffer,
    event::TscRtt,
    time::{Duration, TscCount, tsc::Period},
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
/// 2. If the TSC period has an estimate, then `feed` and then [`Self::expunge_old_events`] to purge old values
///
/// ## Starvation
/// The Local event buffer currently has a simple rejection strategy. Have a multiplier. When attempting to calculate
/// parameters from a new event, if the `RTT(event) >= multiplier * min_rtt`, we reject the sample. We can increase complexity
/// of this with time and exploration of data samples.
///
/// However, this rule **can** cause starvation if the RTT is extremely noisy. That's why the threshold can be tuned
/// or even dynamically modified. Changes subject to experimentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Local<T> {
    /// The inner storage
    inner: RingBuffer<T>,

    /// Rejection threshold when feeding events
    ///
    /// If a value is less than the `min_rtt * rtt_threshold_multiplier`,
    /// then we don't add it
    rtt_threshold_multiplier: usize,

    window: Duration,
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
            window: Self::SKM_WINDOW,
        }
    }

    /// Use an expunge window other than [`Self::SKM_WINDOW`].
    /// TODO: revisit the use of `bon` if valuable. This minimal implementation helps transition
    /// into specializing the variants of the ffsync algorithm.
    #[must_use]
    pub fn with_window(mut self, window: Duration) -> Self {
        self.window = window;
        self
    }

    /// Use an RTT threshold multiplier other than the production default.
    /// TODO: revisit the use of `bon` if valuable. This minimal implementation helps transition
    /// into specializing the variants of the ffsync algorithm.
    #[must_use]
    pub fn with_rtt_threshold_multiplier(mut self, rtt_threshold_multiplier: usize) -> Self {
        self.rtt_threshold_multiplier = rtt_threshold_multiplier;
        self
    }

    pub fn window(&self) -> Duration {
        self.window
    }

    /// Returns an iterator of events from oldest to newest
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.inner.iter()
    }

    /// Returns true if empty
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Clear the internal buffer
    pub fn handle_disruption(&mut self) {
        let Self {
            inner,
            rtt_threshold_multiplier: _rtt,
            window: _,
        } = self;
        inner.clear();
    }

    pub fn rtt_threshold_multiplier(&self) -> usize {
        self.rtt_threshold_multiplier
    }
}

impl<T: TscRtt + Debug> Local<T> {
    /// Feed the ring buffer with a new event
    ///
    /// # Errors
    /// - the event is older than events already in the buffer
    pub fn feed(&mut self, event: T) -> Result<(), FeedError<T>> {
        self.push_if_newer(event)
    }

    /// Adds a new event to the ring buffer
    ///
    /// Returns true if the event was added, false if it was rejected.
    /// The value could be rejected if it is older (via TSC checks) than previous values.
    fn push_if_newer(&mut self, event: T) -> Result<(), FeedError<T>> {
        if let Some(last) = self.inner.head()
            && event.counter_post() <= last.counter_post()
        {
            return Err(FeedError::Old {
                latest_tsc: last.counter_post(),
                event,
            });
        }

        self.inner.push(event);
        Ok(())
    }

    /// Delete events older than the SKM Window
    ///
    /// Requires a period measurement to convert the TSC values to time durations.
    ///
    /// # Starvation warning
    /// This will clear out anything older than the cutoff. This CAN leave the ring buffer EMPTY
    pub fn expunge_old_events(&mut self, period: Period) {
        let Some(head) = self.inner.head() else {
            return; // we empty
        };
        let now_post_tsc = head.counter_post();

        // We need to calculate the corresponding TSC for an SKM window ago
        let cutoff_tsc = now_post_tsc - (self.window / period);

        // If cutoff tsc is negative, that means we booted within an SKM window seconds.
        // Log and bail
        if cutoff_tsc <= TscCount::EPOCH {
            tracing::debug!(%period, ?now_post_tsc, "We booted less than 1 window ago. Not deleting events.");
            return;
        }

        self.inner.expunge_old(cutoff_tsc);
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
        "Event is older than latest event. Buffer's latest event TSC {latest_tsc:?}, event: {event:?}."
    )]
    Old { latest_tsc: TscCount, event: T },
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

        buffer.expunge_old_events(period);

        // Should only retain events within last 1024 seconds
        for event in buffer.iter() {
            assert!(event.counter_post().get() >= 500_000_000_000);
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
            assert_eq!(event.counter_pre().get(), ((i + 2) * 100) as i64);
        }
    }

    #[test]
    fn handle_disruption() {
        let mut local = Local::new(NonZeroUsize::new(3).unwrap());

        let events = vec![
            TestEvent::pre_and_rtt(1000, 100),
            TestEvent::pre_and_rtt(2000, 90),
            TestEvent::pre_and_rtt(3000, 80),
        ];

        for event in &events {
            local.feed(event.clone()).unwrap();
        }

        let thresh = local.rtt_threshold_multiplier;

        assert_eq!(local.inner.len(), 3);

        local.handle_disruption();

        assert_eq!(local.inner.len(), 0);
        assert_eq!(local.rtt_threshold_multiplier, thresh); // unchanged
    }
}
