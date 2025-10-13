//! Ring buffers used internally in the FF clock sync algorithm

use std::{collections::VecDeque, num::NonZeroUsize};

/// A fixed-size ring buffer
///
/// This is largely a wrapper around `VecDeque` while adding protections
/// from arbitrarily growing.
///
/// Uses `head` and `tail` terminology. The head is where the most recent values
/// are added, and `tail` is where the oldest values are. Values are added to the head
/// via [`RingBuffer::push`].
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
        let popped = if self.buffer.len() == self.capacity {
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
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.buffer.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

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
}
