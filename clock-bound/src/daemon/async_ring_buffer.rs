//! Async ring buffer
//!
//! Used for communication from [IO tasks](super::io) to the [Clock Sync Algorithm](super::clock_sync_algorithm).
//!
//! The ring buffer is a single producer single consumer (SPSC) channel.
//!
//! # Receiver
//! Receiving works similar to other async channels, like
//! [tokio's `mpsc::Receiver`](https://docs.rs/tokio/latest/tokio/sync/mpsc/struct.Receiver.html).
//! The `Receiver` can `.await` for more data to be added to the queue, and also get's notified
//! when the sender has dropped.
//!
//! The receiver acts differently from `tokio` in that when the sender closes, the channel closes
//! immediately. It does NOT pull the rest of the messages before closing. This is to better match
//! the needs of ClockBound where we should clean up resources immediately when a source is not reachable.
//!
//! # Sender
//! The `Sender` acts slightly different from
//! [tokio's mpsc::Sender](https://docs.rs/tokio/latest/tokio/sync/mpsc/struct.Sender.html).
//! Because this is a ring-buffer, instead of waiting for capacity, it overwrites the inner value. This
//! means that writing to the sender is never `async`.
//!
//! # Implementation Notes
//! Implementation currently wraps the inner state with an `Arc<Mutex<..>>`. There are more optimized way to do this,
//! but those can come in with time.
//!
//! ## Notifications
//! This implementation uses [`tokio::sync::Notify`] as the async primitive to wake up the [`Receiver::recv`] futures after
//! making state affecting calls to the [`Sender`].
//!
//! General structure is that an `Arc` shares a notify between the [`Sender`] and the [`Receiver`].
//! When the [`Sender`] writes a new value or drops, it notifies the inner `Notify` for the receiver
//! to wake up and handle the update.
//!
//! # Panics
//! Code in this module will panic if called outside of a tokio runtime

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use tokio::sync::Notify;

/// Create a new Sender-Receiver pair async ring buffer
///
/// See [module level documentation](self) for more information.
///
/// # Panics
/// Panics if size is 0
pub fn create<T>(size: usize) -> (Sender<T>, Receiver<T>) {
    assert!(size > 0, "Ring buffer size must be greater than 0");
    let buffer = Arc::new(Mutex::new(Buffer::new(size)));
    let notifier = Arc::new(Notify::new());
    let tx = Sender::new(Arc::clone(&buffer), Arc::clone(&notifier));
    let rx = Receiver::new(buffer, notifier);
    (tx, rx)
}

/// The sender half of a ring buffer SPSC
///
/// See the [module documentation](self) for more information.
#[derive(Debug)]
pub struct Sender<T> {
    inner: Arc<Mutex<Buffer<T>>>,
    notifier: Arc<Notify>,
}

impl<T> Sender<T> {
    fn new(buffer: Arc<Mutex<Buffer<T>>>, notifier: Arc<Notify>) -> Self {
        Self {
            inner: buffer,
            notifier,
        }
    }

    /// Send a value to the ring buffer, overwriting the oldest value if full
    ///
    /// # Errors
    /// Returns [`BufferClosedError`] if the receiver dropped, and therefore nothing
    /// is available to receive messages.
    #[expect(clippy::missing_panics_doc, reason = "not handling poisoned mutex")]
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        let mut guard = self.inner.lock().unwrap();
        if guard.receiver_dropped {
            return Err(BufferClosedError.into());
        }
        if let Some(Side::Receiver) = guard.disruption_handled {
            return Err(SendError::Disrupted(value));
        }
        guard.push(value);
        drop(guard);
        self.notifier.notify_one();
        Ok(())
    }

    /// Handle a clock disruption event
    ///
    /// This clears the internal buffer and leaves a marker that sender has handled it.
    pub fn handle_disruption(&self) {
        #[expect(clippy::missing_panics_doc, reason = "not handling poisoned mutex")]
        self.inner.lock().unwrap().handle_disruption_sender();
    }

    /// Return true if the buffer is empty
    #[expect(clippy::missing_panics_doc, reason = "not handling poisoned mutex")]
    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().is_empty()
    }

    /// Returns `true` if the receiver has dropped, and therefore the channel is closed
    #[expect(clippy::missing_panics_doc, reason = "not handling poisoned mutex")]
    pub fn is_closed(&self) -> bool {
        self.inner.lock().unwrap().receiver_dropped
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        {
            // brace drops guard
            let mut guard = self.inner.lock().unwrap();
            guard.sender_dropped = true;
        }
        self.notifier.notify_one();
    }
}

/// The receiver half of a ring buffer SPSC
///
/// See the [module documentation](self) for more information.
#[derive(Debug)]
pub struct Receiver<T> {
    inner: Arc<Mutex<Buffer<T>>>,
    notifiee: Arc<Notify>,
}

impl<T> Receiver<T> {
    fn new(buffer: Arc<Mutex<Buffer<T>>>, notifiee: Arc<Notify>) -> Self {
        Self {
            inner: buffer,
            notifiee,
        }
    }

    /// Receives the next value for this receiver
    ///
    /// # Errors
    /// This method returns [`BufferClosedError`] if the paired [`Sender`] has dropped (destructed).
    /// This can be used as a signal to clean up paired resources on this side of hte channel.
    ///
    /// # Cancel safety
    /// This method is cancel safe.
    /// If recv is used as the event in a `tokio::select!` statement and some other branch completes first,
    /// it is guaranteed that no messages were received on this channel.
    #[expect(clippy::missing_panics_doc, reason = "not handling poisoned mutex")]
    pub async fn recv(&self) -> Result<T, BufferClosedError> {
        // loop to check values, then await for notification, then get value again
        loop {
            {
                // brace drops guard
                let mut guard = self.inner.lock().unwrap();
                if guard.sender_dropped {
                    return Err(BufferClosedError);
                }
                if let Some(Side::Sender) = guard.disruption_handled {
                    // it's a bug for this to repeatedly fire from the same channel
                    tracing::debug!("Receiving when sender handled disruption");
                }
                if let Some(value) = guard.pop() {
                    return Ok(value);
                }
            }
            self.notifiee.notified().await;
        }
    }

    /// Handle a clock disruption event
    ///
    /// This clears the internal buffer and leaves a marker that the receiver has handled it.
    pub fn handle_disruption(&self) {
        #[expect(clippy::missing_panics_doc, reason = "not handling poisoned mutex")]
        self.inner.lock().unwrap().handle_disruption_receiver();
    }

    /// Returns `true` if the sender has dropped, and therefore the channel is closed
    #[expect(clippy::missing_panics_doc, reason = "not handling poisoned mutex")]
    pub fn is_closed(&self) -> bool {
        self.inner.lock().unwrap().sender_dropped
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut guard = self.inner.lock().unwrap();
        guard.receiver_dropped = true;
    }
}

/// Shared data between the paired [`Tx`] and [`Rx`]
#[derive(Debug)]
struct Buffer<T> {
    data: VecDeque<T>,
    capacity: usize,
    sender_dropped: bool,
    receiver_dropped: bool,
    disruption_handled: Option<Side>,
}

impl<T> Buffer<T> {
    fn new(capacity: usize) -> Self {
        Self {
            data: VecDeque::with_capacity(capacity),
            capacity,
            sender_dropped: false,
            receiver_dropped: false,
            disruption_handled: None,
        }
    }

    /// Pushes a new value into the buffer, overwriting the oldest value if the buffer is full
    ///
    /// Returns the value that was overwritten, if any
    fn push(&mut self, value: T) {
        if self.data.len() == self.capacity {
            self.data.pop_front();
        }
        self.data.push_back(value);
    }

    /// Pops a value from the tail
    ///
    /// Used to remove stale values. Returns `None` if the values are empty
    fn pop(&mut self) -> Option<T> {
        self.data.pop_front()
    }

    /// Returns `true` if the buffer is empty
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    fn handle_disruption_sender(&mut self) {
        match self.disruption_handled {
            None => {
                // not handled yet. Clear the buffer
                self.data.clear();
                self.disruption_handled = Some(Side::Sender);
            }
            Some(Side::Sender) => tracing::warn!("handle disruption sender called multiple times"),
            Some(Side::Receiver) => {
                // already handled. Clear disruption_handled flag
                self.disruption_handled = None;
            }
        }
    }

    fn handle_disruption_receiver(&mut self) {
        match self.disruption_handled {
            None => {
                // not handled yet. Clear the buffer
                self.data.clear();
                self.disruption_handled = Some(Side::Receiver);
            }
            Some(Side::Sender) => {
                // already handled. Clear disruption_handled flag
                self.disruption_handled = None;
            }
            Some(Side::Receiver) => {
                tracing::warn!("handle disruption receiver called multiple times");
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Sender,
    Receiver,
}

#[derive(Debug, thiserror::Error)]
pub enum SendError<T> {
    #[error(transparent)]
    BufferClosed(#[from] BufferClosedError),
    #[error("Send when disrupted")]
    Disrupted(T),
}

#[derive(Debug, thiserror::Error)]
#[error("Buffer has been closed")]
pub struct BufferClosedError;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_buffer() {
        let (tx, _rx) = create::<usize>(5);
        assert!(tx.is_empty());
    }

    #[tokio::test]
    async fn basic_send_receive() {
        let (tx, rx) = create(2);
        tx.send(1).unwrap();
        tx.send(2).unwrap();

        assert_eq!(rx.recv().await.unwrap(), 1);
        assert_eq!(rx.recv().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn buffer_overflow() {
        let (tx, rx) = create(2);
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        tx.send(3).unwrap(); // This should overwrite the oldest value (1)

        assert_eq!(rx.recv().await.unwrap(), 2); // First value (1) was overwritten
        assert_eq!(rx.recv().await.unwrap(), 3);
    }

    #[tokio::test]
    async fn sender_drop() {
        let (tx, rx) = create::<i32>(2);
        tx.send(1).unwrap();
        drop(tx);

        assert!(rx.is_closed());
        // Should receive BufferClosedError after sender is dropped
        assert!(rx.recv().await.is_err());
    }

    #[tokio::test]
    async fn receiver_drop() {
        let (tx, rx) = create::<i32>(2);
        drop(rx);

        assert!(tx.is_closed());
        // Should receive BufferClosedError when trying to send after receiver is dropped
        assert!(tx.send(1).is_err());
    }

    #[tokio::test]
    async fn empty_buffer() {
        let (tx, _rx) = create::<i32>(2);
        assert!(tx.is_empty());

        tx.send(1).unwrap();
        assert!(!tx.is_empty());
    }

    #[tokio::test]
    async fn concurrent_send_receive() {
        let (tx, rx) = create(3);
        let tx_notified = Arc::new(Notify::new());
        let rx_notified = Arc::clone(&tx_notified);

        let handle = tokio::spawn(async move {
            for i in 0..5 {
                tx.send(i).unwrap();
                tx_notified.notified().await;
            }
        });

        let mut received = Vec::new();
        for _ in 0..5 {
            if let Ok(value) = rx.recv().await {
                received.push(value);
                rx_notified.notify_one();
            }
        }

        assert_eq!(received.len(), 5);
        // Check that values are in sequence (though not necessarily starting from 0
        // due to potential overwrites)
        for i in 1..received.len() {
            assert_eq!(received[i], i);
        }
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn cancel_safety() {
        let (tx, rx) = create(2);
        tx.send(1).unwrap();

        tokio::select! {
            biased;
            _ = async {} => {
                // The empty branch should complete first
            }
            _ = rx.recv() => {
                panic!("This branch should not complete first");
            }
        }

        // The value should still be available
        assert_eq!(rx.recv().await.unwrap(), 1);
    }

    // nothing async, but needs tokio runtime due to inner notify
    #[tokio::test]
    async fn handle_disruption_sender_first() {
        let (tx, rx) = create(2);
        tx.send(1).unwrap();

        tx.handle_disruption();
        {
            let inner = tx.inner.lock().unwrap();
            assert!(inner.data.is_empty());
            assert_eq!(inner.disruption_handled, Some(Side::Sender));
        }
        rx.handle_disruption();
        let inner = rx.inner.lock().unwrap();
        assert!(inner.data.is_empty());
        assert_eq!(inner.disruption_handled, None);
    }

    #[tokio::test]
    async fn handle_disruption_receiver_first() {
        let (tx, rx) = create(2);
        tx.send(1).unwrap();

        rx.handle_disruption();
        {
            let inner = rx.inner.lock().unwrap();
            assert!(inner.data.is_empty());
            assert_eq!(inner.disruption_handled, Some(Side::Receiver));
        }
        tx.handle_disruption();
        let inner = tx.inner.lock().unwrap();
        assert!(inner.data.is_empty());
        assert_eq!(inner.disruption_handled, None);
    }

    #[tokio::test]
    async fn handle_disruption_send_after_receive_handles() {
        let (tx, rx) = create(2);
        tx.send(1).unwrap();

        // this should clear
        rx.handle_disruption();

        // this should still send
        let res = tx.send(42);
        let Err(SendError::Disrupted(val)) = &res else {
            panic!("Expected send to be disrupted {res:?}");
        };

        assert_eq!(*val, 42);
    }

    #[tokio::test]
    async fn sender_handles_disruption_while_recv() {
        let (tx, rx) = create(2);
        let recv_fut = rx.recv();

        tokio::select! {
            biased;
            _ = recv_fut => {
                panic!("This branch should not complete first");
            }
            _ = async {
                tx.handle_disruption();
                tx.send(5).unwrap();
            } => {
                // this branch should complete first
            }
        }

        let received = rx.recv().await.unwrap();
        assert_eq!(received, 5);

        // this should clear
        rx.handle_disruption();
    }
}
