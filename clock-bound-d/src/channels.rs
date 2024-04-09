// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::mpsc;

/// Create a web of MPSC channels.
///
/// Given a list of channel IDs, this function creates MPSC channels and allocate the receiver and
/// sender end to the channel to the MailBox and DispatchBox respectively. The MailBox is intended
/// to be consumed and each entry passed to a specific thread to receive on. The DispatchBox is
/// intended to be cloned and passed to each thread to send messages. This strategy implements a
/// "full mesh" communication where each thread can talk to any other thread.
pub fn new_channel_web<K, M>(channel_ids: Vec<K>) -> (MailBox<K, M>, DispatchBox<K, M>)
where
    K: Hash + Eq + Clone,
{
    // Allocate the hashmap holding the channel id and rx/tx ends
    let mut mailbox = HashMap::with_capacity(channel_ids.len());
    let mut dispatchbox = HashMap::with_capacity(channel_ids.len());

    // Create the channels and distribute each of the rx/tx in MailBox/DispatchBox structs
    for id in channel_ids {
        let (sender, receiver) = mpsc::channel();
        mailbox.insert(id.clone(), receiver);
        dispatchbox.insert(id.clone(), sender);
    }

    (
        MailBox { channels: mailbox },
        DispatchBox {
            channels: dispatchbox,
        },
    )
}

/// A collection receiving end of MPSC channels in the communication web.
///
/// This struct is primarily here to retrieve the receiving end of a MPSC channel that has been
/// constructed in this fully meshed communication web. Common use is to build all channels and
/// consume this structure, passing each receiving end of a channel to the matching thread.
pub struct MailBox<K, M>
where
    K: Hash + Eq,
{
    channels: HashMap<K, mpsc::Receiver<M>>,
}

impl<K, M> MailBox<K, M>
where
    K: Hash + Eq,
{
    /// Retrieve the mailbox, that is the receiving end of a MPSC channel a thread can receive on.
    pub fn get_mailbox(&mut self, channel_id: &K) -> Option<mpsc::Receiver<M>> {
        self.channels.remove(channel_id)
    }
}

/// A collection holding the sending ends of all MPSC channels in the communication web.
///
/// The DispatchBox maintains a list of all channels in the communication web, and is used to
/// dispatch a message based on the channel id. Common use is to build all channels and pass a
/// clone of this structure to each thread.
#[derive(Clone)]
pub struct DispatchBox<K, M>
where
    K: Hash + Eq,
{
    channels: HashMap<K, mpsc::Sender<M>>,
}

impl<K, M> DispatchBox<K, M>
where
    K: Hash + Eq,
{
    /// Send a message to a specific channel
    ///
    /// Write a message to a MPSC channel identified by channel_id
    pub fn send(&self, channel_id: &K, message: M) -> Result<(), mpsc::SendError<M>> {
        match self.channels.get(channel_id) {
            Some(sender) => sender.send(message),
            None => Err(mpsc::SendError(message)),
        }
    }

    /// Return the number of channels in the DispatchBox.
    pub fn keys(&self) -> std::collections::hash_map::Keys<'_, K, mpsc::Sender<M>> {
        self.channels.keys()
    }
}

#[cfg(test)]
mod t_channels {
    use super::*;

    impl<K, M> MailBox<K, M>
    where
        K: Hash + Eq,
    {
        /// Return the number of channels in the MailBox.
        fn len(&self) -> usize {
            self.channels.len()
        }
    }

    impl<K, M> DispatchBox<K, M>
    where
        K: Hash + Eq,
    {
        /// Return the number of channels in the DispatchBox.
        fn len(&self) -> usize {
            self.channels.len()
        }
    }

    // Assert that one can send messages between channels using the MailBox and DispatchBox structs
    #[test]
    fn test_new_channels_web() {
        // Build a web of channels
        let channel_ids = vec!["foo", "bar"];
        let (mut mbox, dbox) = new_channel_web(channel_ids);

        // Send to foo
        dbox.send(&"foo", "hello").unwrap();
        let rx = mbox.get_mailbox(&"foo").unwrap();
        let message = rx.recv().unwrap();
        assert_eq!(message, "hello");

        // Send to bar
        dbox.send(&"bar", "world").unwrap();
        let rx = mbox.get_mailbox(&"bar").unwrap();
        let message = rx.recv().unwrap();
        assert_eq!(message, "world");
    }

    // Assert that no duplicate channels is built in the channel web.
    #[test]
    fn test_no_duplicate_channels() {
        // Build a web of channels
        let channel_ids = vec!["foo", "foo", "foo"];
        let (mbox, dbox) = new_channel_web(channel_ids);

        // Cheeky way to let the compiler infer the right type for the message
        dbox.send(&"foo", "hello").unwrap();

        assert_eq!(mbox.len(), 1);
        assert_eq!(dbox.len(), 1);
    }

    // Assert that no duplicate mailbox is extracted from the channel web.
    #[test]
    fn test_no_duplicate_mailboxes() {
        // Build a web of channels
        let channel_ids = vec!["foo"];
        let (mut mbox, _dbox) = new_channel_web(channel_ids);

        let _rx: mpsc::Receiver<&str> = mbox.get_mailbox(&"foo").unwrap();
        assert!(mbox.get_mailbox(&"foo").is_none());
    }
}
