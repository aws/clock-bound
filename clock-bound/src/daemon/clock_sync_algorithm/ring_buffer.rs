//! Ring buffers used internally in the FF clock sync algorithm

use std::{collections::VecDeque, fmt::Debug, num::NonZeroUsize};

use crate::daemon::{event::TscRtt, time::TscCount};

/// A fixed-size ring buffer
///
/// This is largely a wrapper around `VecDeque` while adding protections
/// from arbitrarily growing.
///
/// Uses `head` and `tail` terminology. The head is where the most recent values
/// are added, and `tail` is where the oldest values are. Values are added to the head
/// via [`RingBuffer::push`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingBuffer<T> {
    buffer: VecDeque<T>,
    capacity: usize,
}

impl<T> RingBuffer<T> {
    /// Creates a new ring buffer with the given capacity
    pub fn new(capacity: NonZeroUsize) -> Self {
        let capacity = capacity.get();
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Pushes a new value into the buffer, overwriting the oldest value if the buffer is full
    ///
    /// Returns the value that was overwritten, if any
    pub fn push(&mut self, value: T) -> Option<T> {
        let popped = if self.is_full() {
            self.buffer.pop_front()
        } else {
            None
        };
        self.buffer.push_back(value);
        popped
    }

    /// Pops a value from the tail
    ///
    /// Used to remove stale values. Returns `None` if the values are empty
    pub fn pop(&mut self) -> Option<T> {
        self.buffer.pop_front()
    }

    /// Returns the value at the given index, or `None` if the index is out of bounds
    pub fn peek_at(&self, index: usize) -> Option<&T> {
        self.buffer.get(index)
    }

    /// Returns the number of values in the buffer
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Returns the capacity of the buffer
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns `true` if the buffer is empty
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Returns `true` if the buffer is full
    pub fn is_full(&self) -> bool {
        self.buffer.len() == self.capacity
    }

    /// Clears the buffer
    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Get the latest value added to the ring buffer
    pub fn head(&self) -> Option<&T> {
        self.buffer.back()
    }

    /// Get the oldest value in the ring buffer
    pub fn tail(&self) -> Option<&T> {
        self.buffer.front()
    }

    /// iterate from the tail to the head (oldest to newest)
    ///
    /// Use `rev` on the iterator if you want to search the other way
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.buffer.iter()
    }
}

impl<T: TscRtt> RingBuffer<T> {
    /// Return the value with the lowest rtt in the `ring_buffer`
    pub fn min_rtt(&self) -> Option<&T> {
        self.buffer.iter().min_by_key(|v| v.rtt())
    }

    /// Return the value with the lowest rtt in the specified quarter of the ring buffer.
    ///
    /// This returns `Some` as long as the buffer is not empty.
    #[expect(clippy::missing_panics_doc, reason = "unwraps have checks")]
    pub fn min_rtt_in_quarter(&self, quarter: Quarter) -> Option<&T> {
        if self.is_empty() {
            return None;
        }

        // Identify the element range
        let (start_idx, end_idx) = match quarter {
            Quarter::Oldest => {
                let start_idx = 0;
                // unwraps okay. buffer isn't empty
                let start_pre_tsc = self.tail().unwrap().tsc_pre();
                let end_pre_tsc = self.head().unwrap().tsc_pre();
                let end_pre_tsc = start_pre_tsc + (end_pre_tsc - start_pre_tsc) / 4;
                let mut end_idx = 0;
                for event in self.iter() {
                    if event.tsc_pre() > end_pre_tsc {
                        break;
                    }
                    end_idx += 1;
                }
                (start_idx, end_idx)
            }
            Quarter::Newest => {
                let end_idx = self.len();
                // unwraps okay. buffer isn't empty
                let start_pre_tsc = self.tail().unwrap().tsc_pre();
                let end_pre_tsc = self.head().unwrap().tsc_pre();
                let start_pre_tsc = end_pre_tsc - (end_pre_tsc - start_pre_tsc) / 4;
                let mut start_idx = self.len();
                for event in self.iter().rev() {
                    if event.tsc_pre() < start_pre_tsc {
                        break;
                    }
                    start_idx -= 1;
                }
                (start_idx, end_idx)
            }
        };

        // TODO: this is a second iteration of the buffer. Potential optimization is to find min on first pass
        self.iter()
            .skip(start_idx)
            .take(end_idx - start_idx)
            .min_by_key(|v| v.rtt())
    }
}

impl<T: TscRtt + Debug> RingBuffer<T> {
    /// Purge undesired values
    ///
    /// Removes values from the buffers that:
    /// - Are before a certain TSC
    /// - are greater than a specified rtt
    pub fn expunge_old(&mut self, before: TscCount) {
        self.buffer.retain(|event| {
            // Now, I would normally like to NOT have any logging in a low level function like this
            // But I don't have a way of returning the dropped values without allocating.
            //
            // Logging at a debug level
            if event.tsc_post() >= before {
                true
            } else {
                tracing::debug!(?event, "Purging stale event");
                false
            }
        });
    }
}

/// Specify the quarter in [`RingBuffer::min_rtt_in_quarter`] to use
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quarter {
    Oldest,
    Newest,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    use crate::daemon::{
        clock_sync_algorithm::ff::event_buffer::test_assets::TestEvent, time::TscDiff,
    };

    #[test]
    fn new_buffer() {
        let buffer: RingBuffer<i32> = RingBuffer::new(NonZeroUsize::new(3).unwrap());
        assert_eq!(buffer.capacity(), 3);
        assert_eq!(buffer.len(), 0);
        assert!(buffer.is_empty());
        assert!(!buffer.is_full());
    }

    #[test]
    fn push_and_peek() {
        let mut buffer = RingBuffer::new(NonZeroUsize::new(3).unwrap());

        buffer.push(1);
        buffer.push(2);

        assert_eq!(buffer.peek_at(0), Some(&1));
        assert_eq!(buffer.peek_at(1), Some(&2));
        assert_eq!(buffer.peek_at(2), None);
    }

    #[test]
    fn buffer_overflow() {
        let mut buffer = RingBuffer::new(NonZeroUsize::new(2).unwrap());

        buffer.push(1);
        buffer.push(2);
        buffer.push(3);

        assert_eq!(buffer.peek_at(0), Some(&2));
        assert_eq!(buffer.peek_at(1), Some(&3));
        assert_eq!(buffer.peek_at(2), None);
    }

    #[test]
    fn wrap() {
        let mut buffer = RingBuffer::new(NonZeroUsize::new(2).unwrap());

        buffer.push(1);
        buffer.push(2);
        buffer.push(3);
    }

    #[test]
    fn head_and_tail() {
        let mut buffer = RingBuffer::new(NonZeroUsize::new(3).unwrap());

        assert_eq!(buffer.head(), None);
        assert_eq!(buffer.tail(), None);

        buffer.push(1);
        assert_eq!(buffer.head(), Some(&1));
        assert_eq!(buffer.tail(), Some(&1));

        buffer.push(2);
        assert_eq!(buffer.head(), Some(&2));
        assert_eq!(buffer.tail(), Some(&1));
    }

    #[test]
    fn clear() {
        let mut buffer = RingBuffer::new(NonZeroUsize::new(3).unwrap());

        buffer.push(1);
        buffer.push(2);
        buffer.clear();

        assert!(buffer.is_empty());
        assert_eq!(buffer.len(), 0);
        assert_eq!(buffer.head(), None);
        assert_eq!(buffer.tail(), None);
    }

    #[test]
    fn iter() {
        let mut buffer = RingBuffer::new(NonZeroUsize::new(3).unwrap());
        buffer.push(1);
        buffer.push(2);
        buffer.push(3);

        let values: Vec<&i32> = buffer.iter().collect();
        assert_eq!(values, vec![&1, &2, &3]);

        // Test iteration after overflow
        buffer.push(4);
        let values: Vec<&i32> = buffer.iter().collect();
        assert_eq!(values, vec![&2, &3, &4]);
    }

    #[rstest]
    #[case(1)]
    #[case(5)]
    #[case(10)]
    fn various_capacities(#[case] capacity: usize) {
        let mut buffer = RingBuffer::new(NonZeroUsize::new(capacity).unwrap());

        for i in 0..capacity {
            buffer.push(i);
            assert_eq!(buffer.len(), i + 1);
        }

        assert!(buffer.is_full());

        // Push one more to test overflow
        buffer.push(capacity);
        assert_eq!(buffer.len(), capacity);
    }

    #[test]
    fn pop() {
        let mut buffer = RingBuffer::new(NonZeroUsize::new(3).unwrap());
        buffer.push(1);
        buffer.push(2);
        buffer.push(3);

        assert_eq!(buffer.pop(), Some(1));
        assert_eq!(buffer.pop(), Some(2));
        assert_eq!(buffer.pop(), Some(3));
        assert_eq!(buffer.pop(), None);
    }

    #[test]
    fn min_rtt() {
        let mut buffer = RingBuffer::new(NonZeroUsize::new(3).unwrap());

        let events = vec![
            TestEvent::pre_and_rtt(100, 30),
            TestEvent::pre_and_rtt(200, 10),
            TestEvent::pre_and_rtt(300, 20),
        ];

        for event in events {
            buffer.push(event);
        }

        let min_rtt = buffer.min_rtt().unwrap();
        assert_eq!(min_rtt.rtt(), TscDiff::new(10));
    }

    #[test]
    fn min_rtt_in_quarter_newest() {
        let mut buffer = RingBuffer::new(NonZeroUsize::new(4).unwrap());

        let events = vec![
            TestEvent::pre_and_rtt(100, 40),
            TestEvent::pre_and_rtt(200, 20),
            TestEvent::pre_and_rtt(300, 30),
            TestEvent::pre_and_rtt(400, 50),
        ];

        for event in events {
            buffer.push(event);
        }

        let min_rtt = buffer.min_rtt_in_quarter(Quarter::Newest).unwrap();
        assert_eq!(min_rtt.rtt(), TscDiff::new(50));
    }

    #[test]
    fn min_rtt_in_quarter_oldest() {
        let mut buffer = RingBuffer::new(NonZeroUsize::new(4).unwrap());

        let events = vec![
            TestEvent::pre_and_rtt(100, 60),
            TestEvent::pre_and_rtt(200, 20),
            TestEvent::pre_and_rtt(300, 30),
            TestEvent::pre_and_rtt(400, 40),
        ];

        for event in events {
            buffer.push(event);
        }

        let min_rtt = buffer.min_rtt_in_quarter(Quarter::Oldest).unwrap();
        assert_eq!(min_rtt.rtt(), TscDiff::new(60));
    }

    #[test]
    fn min_rtt_in_quarter_2_values() {
        let mut buffer = RingBuffer::new(NonZeroUsize::new(4).unwrap());

        let events = vec![
            TestEvent::pre_and_rtt(100, 60),
            TestEvent::pre_and_rtt(200, 20),
        ];

        for event in events {
            buffer.push(event);
        }

        let min_rtt = buffer.min_rtt_in_quarter(Quarter::Oldest).unwrap();
        assert_eq!(min_rtt.rtt(), TscDiff::new(60));

        let min_rtt = buffer.min_rtt_in_quarter(Quarter::Newest).unwrap();
        assert_eq!(min_rtt.rtt(), TscDiff::new(20));
    }

    #[test]
    fn purge_earlier_than() {
        let mut buffer = RingBuffer::new(NonZeroUsize::new(3).unwrap());

        let events = vec![
            TestEvent::pre_and_rtt(100, 10),
            TestEvent::pre_and_rtt(200, 20),
            TestEvent::pre_and_rtt(300, 30),
        ];

        for event in events {
            buffer.push(event);
        }

        buffer.expunge_old(TscCount::new(250));
        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer.head().unwrap().tsc_post(), TscCount::new(330));
    }

    #[test]
    fn empty_buffer_operations() {
        let mut buffer: RingBuffer<TestEvent> = RingBuffer::new(NonZeroUsize::new(3).unwrap());

        assert_eq!(buffer.min_rtt(), None);
        assert_eq!(buffer.min_rtt_in_quarter(Quarter::Newest), None);
        assert_eq!(buffer.min_rtt_in_quarter(Quarter::Oldest), None);

        // Purging an empty buffer should not panic
        buffer.expunge_old(TscCount::new(100));
        assert!(buffer.is_empty());
    }

    #[test]
    fn single_element_buffer() {
        let mut buffer = RingBuffer::new(NonZeroUsize::new(1).unwrap());

        let event = TestEvent::pre_and_rtt(100, 10);
        buffer.push(event);

        assert!(buffer.is_full());
        assert_eq!(buffer.len(), 1);

        // Both quarters should return the same element
        let newest = buffer.min_rtt_in_quarter(Quarter::Newest).unwrap().rtt();
        let oldest = buffer.min_rtt_in_quarter(Quarter::Oldest).unwrap().rtt();
        assert_eq!(newest, oldest);
    }

    #[test]
    fn overflow_behavior() {
        let mut buffer = RingBuffer::new(NonZeroUsize::new(2).unwrap());

        let events = vec![
            TestEvent::pre_and_rtt(100, 10),
            TestEvent::pre_and_rtt(200, 20),
            TestEvent::pre_and_rtt(300, 30),
        ];

        for event in events {
            buffer.push(event);
        }

        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.tail().unwrap().tsc_pre(), TscCount::new(200));
        assert_eq!(buffer.head().unwrap().tsc_pre(), TscCount::new(300));
    }
}
