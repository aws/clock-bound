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
        if self.inner.is_empty() {
            return;
        }
        match self.cutoff(self.window, period) {
            Some(cutoff_tsc) => self.inner.expunge_old(cutoff_tsc),
            // A cutoff before the epoch means we booted within one window. Log and bail.
            None => {
                tracing::debug!(%period, ?self.window, "We booted less than 1 window ago. Not deleting events.");
            }
        }
    }

    /// The counter value `window` before the newest event, or `None` when that instant falls at
    /// or before the epoch (the machine booted less than one window ago) or the buffer is empty.
    ///
    /// Requires a period measurement to convert the window to counter ticks. The same cutoff
    /// serves [`Self::expunge_old_events`] and [`Self::window_start`], so the two agree.
    pub fn cutoff(&self, window: Duration, period: Period) -> Option<TscCount> {
        let now_post_tsc = self.inner.head()?.counter_post();
        let cutoff_tsc = now_post_tsc - (window / period);
        (cutoff_tsc > TscCount::EPOCH).then_some(cutoff_tsc)
    }

    /// Index (oldest first) where the last `window` of events begins.
    ///
    /// A window at least as long as this buffer's own window is the whole buffer: `0`, with no
    /// cutoff computed at all, so that the production configuration (windows equal to the buffer)
    /// behaves exactly as before the windows existed. A shorter window narrows the view to the
    /// events whose `counter_post` is at or after the cutoff; a cutoff before the epoch also means
    /// the whole buffer.
    pub fn window_start(&self, window: Duration, period: Period) -> usize {
        if window >= self.window {
            return 0;
        }
        match self.cutoff(window, period) {
            Some(cutoff_tsc) => self.inner.index_at_or_after(cutoff_tsc),
            None => 0,
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

    #[test]
    fn cutoff_is_window_before_head_or_none_near_epoch() {
        let period = Period::from_seconds(1e-9); // 1 GHz: 1 tick = 1 ns
        let mut local: Local<TestEvent> = Local::new(NonZeroUsize::new(8).unwrap());
        assert!(local.cutoff(Duration::from_secs(1), period).is_none()); // empty
        local
            .feed(TestEvent::new(5_000_000_000, 5_000_000_100))
            .unwrap(); // head post = 5 s
        assert_eq!(
            local.cutoff(Duration::from_secs(2), period),
            Some(TscCount::new(3_000_000_100))
        );
        // 5 s back from 5 s + 100 ns leaves 100 ticks: still after the epoch.
        assert_eq!(
            local.cutoff(Duration::from_secs(5), period),
            Some(TscCount::new(100))
        );
        // A window reaching the epoch or before: None ("booted less than one window ago").
        assert!(
            local
                .cutoff(Duration::from_secs(5) + Duration::from_nanos(100), period)
                .is_none()
        );
        assert!(local.cutoff(Duration::from_secs(6), period).is_none());
    }

    #[test]
    fn window_start_whole_buffer_when_window_is_not_shorter() {
        let period = Period::from_seconds(1e-9);
        let mut local: Local<TestEvent> =
            Local::new(NonZeroUsize::new(8).unwrap()).with_window(Duration::from_secs(100));
        for s in 1..=5_u64 {
            local
                .feed(TestEvent::new(s * 1_000_000_000, s * 1_000_000_000 + 100))
                .unwrap();
        }
        // Equal to or longer than the buffer's window: index 0, regardless of the data.
        assert_eq!(local.window_start(Duration::from_secs(100), period), 0);
        assert_eq!(local.window_start(Duration::from_secs(1024), period), 0);
    }

    #[test]
    fn window_start_narrows_to_the_last_window() {
        let period = Period::from_seconds(1e-9);
        let mut local: Local<TestEvent> =
            Local::new(NonZeroUsize::new(8).unwrap()).with_window(Duration::from_secs(100));
        // Events at 10 s, 11 s, ..., 15 s (post = pre + 100 ticks).
        for s in 10..=15_u64 {
            local
                .feed(TestEvent::new(s * 1_000_000_000, s * 1_000_000_000 + 100))
                .unwrap();
        }
        // Last 2 s before head post (15 s + 100 ns): cutoff 13 s + 100 ns -> events at 14 s, 15 s
        // (the 13 s event's post equals the cutoff and is kept: same rule as expunge).
        assert_eq!(local.window_start(Duration::from_secs(2), period), 3);
        // A window shorter than the spacing: only the head.
        assert_eq!(local.window_start(Duration::from_millis(500), period), 5);
        // A window reaching before the epoch: the whole buffer.
        assert_eq!(local.window_start(Duration::from_secs(20), period), 0);
    }
}
