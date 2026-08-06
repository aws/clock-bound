//! An estimate event buffer
use std::num::NonZeroUsize;

use super::Local;
use crate::daemon::{
    clock_sync_algorithm::RingBuffer,
    event::TscRtt,
    time::{TscCount, tsc::Period},
};

/// An estimate ring buffer
///
/// # Intended use
/// Constantly feed a [`Local`] estimate buffer and then feed the [`Estimate`] with
/// [`Estimate::feed`]. This will do nothing in most calls, but if the SKM window has expired,
/// it will take the lowest RTT value in [`Local`] and push it into [`Estimate`].
///
/// This way a long term estimate on the TSC period can be done.
///
/// It is expected that the same [`Local`] buffer is passed into a constructed [`Estimate`] for the lifetime
/// of both buffers (they are eternally linked).
///
/// # Storage and capacity
/// This one stores the best RTT values of each SKM window
/// over a longer period of time (upwards of 1 week)
///
/// Generally the capacity of this will remain static, as we need to hold 1 week of
/// data where each sample is the best datapoint in an SKM window. This is roughly
/// a datapoint every 1,000 seconds for 604,800 seconds, leading to ~600 data points max.
///
/// Note that this buffer is not strictly time-bound. If a SKM window is completely starved for
/// hours (days!), it still has a capacity of 600 and will end up storing longer periods of time.
/// Samples don't have a concept of expiring, we are just trading off longer duration estimates
/// with memory.
///
/// # Windowing
/// When *attempting* to add a new event to the [`Local`] ring buffer, if it is greater than
/// 1000 seconds since the last time the [`Estimate`] ring buffer was updated, we pick the min
/// rtt value from the [`Local`] ring buffer, and push that onto the [`Estimate`] ring buffer.
///
/// When we search for the min rtt value, we exclude the value just added to the [`Local`] ring
/// buffer as that value is outside the SKM window.
///
/// # Initialization
/// This struct will initialize on the first call to [`Estimate::feed`] with the oldest timestamp in the [`Local`]
/// buffer.
///
/// # Starvation
/// What happens if there are NO good data points for an SKM? That's an issue for [`Local`] to handle. But windows without
/// anything in Local will just early exit, as there is nothing to feed into [`Estimate`]
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(bon::Builder))] // Workaround the strict invariants that make testing un-ergonomic
pub struct Estimate<T> {
    /// The inner storage
    inner: RingBuffer<T>,
    /// The `counter_post` of the last event we added to the ring buffer
    ///
    /// Value is `Some` if there are values in `self.inner` OR
    /// this has been initialized in an early [`Estimate::feed`] call
    last_counter_post: Option<TscCount>,
}

impl<T> Estimate<T> {
    // Hold data from 600 SKM windows
    const CAPACITY: NonZeroUsize = NonZeroUsize::new(600).unwrap();

    /// Construct
    pub fn new() -> Self {
        Self {
            inner: RingBuffer::new(Self::CAPACITY),
            last_counter_post: None,
        }
    }

    /// get the `last_counter_post`
    ///
    /// None if no data was ever in the paired `Local` on feed
    pub fn last_counter_post(&self) -> Option<TscCount> {
        self.last_counter_post
    }

    /// get the length
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// returns true if the buffer is empty
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns an iterator of events from oldest to newest
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.inner.iter()
    }

    #[cfg(test)]
    fn push(&mut self, event: T, now_counter_post: TscCount) {
        self.last_counter_post = Some(now_counter_post);
        self.inner.push(event);
    }

    /// Clear the internal buffer
    pub fn handle_disruption(&mut self) {
        let Self {
            inner,
            last_counter_post,
        } = self;
        inner.clear();
        *last_counter_post = None;
    }
}

impl<T: TscRtt + Clone> Estimate<T> {
    /// Feed the ring buffer IF the SKM window expired
    ///
    /// Given a [`Local`] ring buffer that has been [`Estimate::feed`] with an event at `now_event_counter_post`,
    /// update the [`Estimate`] ring buffer if the SKM window has expired
    ///
    /// Returns `Some` with the updated value if an event was fed in. `None` otherwise.
    ///
    /// # Period input
    /// Updating the estimate event buffer requires a period measurement. This is because this buffer
    /// is inherently time based (it stores the best result of every 1024 second window, which assumes
    /// we know the tsc rate..).
    ///
    /// This means the `estimate` event buffer cannot start to be populated until we have an initial period calculation.
    ///
    /// ## Starvation
    /// Ideally the local buffer is relatively full of samples by the time the SKM window expires. However, if the local
    /// buffer is empty for an entire SKM window, that data point will simply skip.
    #[expect(clippy::missing_panics_doc, reason = "unwraps commented")]
    pub fn feed(&mut self, local: &Local<T>, period_estimate: Period) -> Option<&T> {
        // bail out if local is empty.
        let head = local.as_ref().head()?;
        let now_event_counter_post = head.counter_post();

        let Some(last_counter_post) = self.last_counter_post else {
            // We are not initialized. Initialize and bail out.
            // unwrap okay. local confirmed as not empty above
            let tail_counter_post = local.as_ref().tail().unwrap().counter_post();
            self.last_counter_post = Some(tail_counter_post);
            return None;
        };

        let diff = now_event_counter_post - last_counter_post;
        let duration = diff * period_estimate;

        if duration < local.window() {
            // SKM window hasn't expired yet. Bail out
            None
        } else {
            // find the min rtt value in the local buffer
            // exclude the value we just added
            //
            // rev() because iter goes from oldest to newest
            let mut rev_iter = local.iter().rev().peekable();

            if let Some(event) = rev_iter.peek()
                && event.counter_post() == now_event_counter_post
            {
                // if this is the value we just added skip
                rev_iter.next();
            }

            let min_rtt_event = rev_iter.min_by_key(|event| event.rtt());

            let Some(min_rtt_event) = min_rtt_event else {
                // could happen if we call this function with a single value in the local buffer,
                tracing::warn!(
                    "only the latest value found in Local estimate buffer. We are likely starving."
                );
                self.last_counter_post = Some(now_event_counter_post);
                // unwrap ok. local is not empty
                self.inner.push(local.as_ref().head().unwrap().clone());
                return self.inner.head();
            };

            // push and store this value
            self.last_counter_post = Some(now_event_counter_post);
            self.inner.push(min_rtt_event.clone());
            self.inner.head()
        }
    }
}

impl<T> AsRef<RingBuffer<T>> for Estimate<T> {
    fn as_ref(&self) -> &RingBuffer<T> {
        &self.inner
    }
}

impl<T> Default for Estimate<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_assets::TestEvent;
    use super::*;
    use crate::daemon::event::Event;

    #[test]
    fn default_state() {
        let estimate: Estimate<Event> = Estimate::new();
        assert!(estimate.is_empty());
        assert_eq!(estimate.len(), 0);
        assert!(estimate.last_counter_post().is_none());
    }

    #[test]
    fn push_single_event() {
        let mut estimate = Estimate::new();
        let event = TestEvent::pre_and_rtt(1000, 100);
        estimate.push(event.clone(), TscCount::new(1000));

        assert!(!estimate.is_empty());
        assert_eq!(estimate.len(), 1);
        assert_eq!(estimate.last_counter_post(), Some(TscCount::new(1000)));
    }

    #[test]
    fn feed_empty_local() {
        let mut estimate: Estimate<TestEvent> = Estimate::new();
        let local: Local<TestEvent> = Local::new(NonZeroUsize::new(5).unwrap());
        let period = Period::from_seconds(1e-9); // 1GHz for example

        let result = estimate.feed(&local, period);
        assert!(result.is_none());
    }

    #[test]
    fn feed_first_event() {
        let mut estimate: Estimate<TestEvent> = Estimate::new();
        let mut local = Local::new(NonZeroUsize::new(5).unwrap());
        let event = TestEvent::pre_and_rtt(1000, 100);

        // Add event to local buffer
        local.feed(event.clone()).unwrap();

        let period = Period::from_seconds(1e-9); //1 GHz
        let result = estimate.feed(&local, period);

        assert!(result.is_none());
    }

    #[test]
    fn feed_before_window_expiry() {
        let mut estimate: Estimate<TestEvent> = Estimate::new();
        let mut local = Local::new(NonZeroUsize::new(5).unwrap());

        // Add initial event
        let event1 = TestEvent::pre_and_rtt(1000, 100);
        local.feed(event1.clone()).unwrap();
        let period = Period::from_seconds(1e-9);

        // First feed should succeed
        estimate.feed(&local, period);

        // Add another event before window expiry (less than 1000 seconds)
        let event2 = TestEvent::pre_and_rtt(2000, 90);
        local.feed(event2.clone()).unwrap();

        let result = estimate.feed(&local, period);
        assert!(result.is_none());
    }

    #[test]
    fn feed_after_window_expiry() {
        let mut estimate: Estimate<TestEvent> = Estimate::new();
        let mut local = Local::new(NonZeroUsize::new(5).unwrap());
        let period = Period::from_seconds(1e-9);

        // Add initial event
        let event1 = TestEvent::pre_and_rtt(1000, 100);
        local.feed(event1.clone()).unwrap();
        let retval = estimate.feed(&local, period);
        assert!(retval.is_none());

        // Add event after window expiry (> 1000 seconds)
        // Using 2_000_000_000_000 ticks (2000 seconds at 1GHz) to ensure window expiry
        let event2 = TestEvent::pre_and_rtt(2_000_000_000_000, 90);
        local.feed(event2.clone()).unwrap();

        let result = estimate.feed(&local, period);
        let result = result.unwrap();
        assert_eq!(*result, event1);
        assert_eq!(estimate.len(), 1);
    }

    #[test]
    fn feed_happy_path() {
        let mut estimate: Estimate<TestEvent> = Estimate::new();
        let mut local = Local::new(NonZeroUsize::new(200).unwrap());
        let period = Period::from_seconds(1e-9);

        // Add initial event to estimate
        let event1 = TestEvent::pre_and_rtt(1000, 100);
        local.feed(event1.clone()).unwrap();
        estimate.feed(&local, period);

        let events: Vec<_> = (1..=100)
            .map(|i| {
                //local buffer gets events from 926 seconds to 1025. Last one triggers
                TestEvent::pre_and_rtt((i + 925) * 1_000_000_000, 100)
            })
            .collect();

        // this is the min rtt event. This should get pushed to estimate buffer after the SKM window expires (last element in vec)
        let expected = TestEvent::pre_and_rtt(500 * 1_000_000_000, 50);
        let events = {
            let mut tmp = vec![expected.clone()];
            tmp.extend(events);
            tmp
        };

        // Feed everything but the last event. Estimate feed should return `None` since SKM never expired
        for i in 0..100 {
            // not the last value
            local.feed(events[i].clone()).unwrap();
            local.expunge_old_events(period);
            let result = estimate.feed(&local, period);
            assert!(
                result.is_none(),
                "Failure at event {i}, {:?}",
                events[i].clone()
            );
        }

        // last event triggers the 1024 second SKM window expiring.
        local.feed(events[100].clone()).unwrap();
        local.expunge_old_events(period);
        let result = estimate.feed(&local, period);
        let result = result.unwrap();
        assert_eq!(*result, expected);
    }

    #[test]
    fn feed_newest_excluded() {
        let mut estimate: Estimate<TestEvent> = Estimate::new();
        let mut local = Local::new(NonZeroUsize::new(200).unwrap());
        let period = Period::from_seconds(1e-9);

        // Add initial event to estimate
        let event1 = TestEvent::pre_and_rtt(1000, 100);
        local.feed(event1.clone()).unwrap();
        estimate.feed(&local, period);

        let events: Vec<_> = (1..=100)
            .map(|i| {
                //local buffer gets events from 926 seconds to 1025. Last one triggers
                TestEvent::pre_and_rtt((i + 925) * 1_000_000_000, 100)
            })
            .collect();

        // this is the min rtt event. This should get pushed to estimate buffer after the SKM window expires (last element in vec)
        let expected = TestEvent::pre_and_rtt(500 * 1_000_000_000, 50);
        let mut events = {
            let mut tmp = vec![expected.clone()];
            tmp.extend(events);
            tmp
        };

        // Overwrite the last event to have the minimum RTT
        events[100] = TestEvent::pre_and_rtt(1025 * 1_000_000_000, 25);

        // Feed everything but the last event. Estimate feed should return `None` since SKM never expired
        for i in 0..100 {
            // not the last value
            local.feed(events[i].clone()).unwrap();
            local.expunge_old_events(period);
            let result = estimate.feed(&local, period);
            assert!(
                result.is_none(),
                "Failure at event {i}, {:?}",
                events[i].clone()
            );
        }

        // last event triggers the 1000 second SKM window expiring.
        local.feed(events[100].clone()).unwrap();
        local.expunge_old_events(period);
        let result = estimate.feed(&local, period);
        let result = result.unwrap();

        // added value should NOT be the overwritten 101th element
        assert_eq!(*result, expected);
    }

    #[test]
    fn iter_ordering() {
        let mut estimate: Estimate<TestEvent> = Estimate::new();

        let events = vec![
            TestEvent::pre_and_rtt(1000, 100),
            TestEvent::pre_and_rtt(2000, 90),
            TestEvent::pre_and_rtt(3000, 80),
        ];

        for event in &events {
            estimate.push(event.clone(), event.counter_post());
        }

        let collected: Vec<_> = estimate.iter().collect();
        assert_eq!(collected.len(), events.len());

        // Verify ordering from oldest to newest
        for (i, event) in collected.iter().enumerate() {
            assert_eq!(event.counter_post(), events[i].counter_post());
        }
    }

    #[test]
    fn handle_disruption() {
        let mut estimate: Estimate<TestEvent> = Estimate::new();

        let events = vec![
            TestEvent::pre_and_rtt(1000, 100),
            TestEvent::pre_and_rtt(2000, 90),
            TestEvent::pre_and_rtt(3000, 80),
        ];

        for event in &events {
            estimate.push(event.clone(), event.counter_post());
        }

        assert_eq!(estimate.len(), 3);
        assert!(estimate.last_counter_post.is_some());

        estimate.handle_disruption();

        assert_eq!(estimate.len(), 0);
        assert!(estimate.last_counter_post.is_none());
    }
}
