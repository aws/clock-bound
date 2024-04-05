// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only

use std::sync::mpsc::Receiver;
use std::thread::{panicking, spawn};
use tracing::{debug, error, info};

use crate::{
    channels::{self, DispatchBox},
    PhcInfo,
};
use crate::{chrony_poller, shm_writer};
use crate::{ChannelId, Message};

/// Context passed to newly spawned threads
///
/// This structure encapsulate context information passed to threads. This include the
/// mailbox/dispatch box MPSC information for threads to communicate, and can be extended.
pub struct Context {
    // The channel identifier for this thread
    pub channel_id: ChannelId,

    // The receiving end of the MPSC channel the thread receives on.
    pub mbox: Receiver<Message>,

    // The DispatchBox to send message to any other thread.
    pub dbox: DispatchBox<ChannelId, Message>,
}

impl Drop for Context {
    /// A context is passed to each thread, implementing Drop gives an opportunity to gracefully shut
    /// everything down, or recover. Before the context is finally dropped, send a message back to
    /// the main thread and let it "do the right thing".
    fn drop(&mut self) {
        // Distinguish between a panic and a "normal" termination of the thread.
        let message = if panicking() {
            Message::ThreadPanic(self.channel_id.clone())
        } else {
            Message::ThreadTerminate(self.channel_id.clone())
        };

        match self.dbox.send(&ChannelId::MainThread, message) {
            Ok(()) => debug!(
                "Thread {:?} signalled the main thread it Drop'ed ",
                self.channel_id
            ),
            Err(_) => error!(
                "Thread {:?} failed to signal back to the main thread",
                self.channel_id
            ),
        }
    }
}

/// Send an Abort message to all threads
///
/// Iterate over the DispatchBox to send an Abort message to every thread. This effectively asks
/// every thread to terminate gracefully.
fn broadcast_abort(dispatchbox: DispatchBox<ChannelId, Message>) {
    // Note that the main thread is filtered out (no need to signal to it), but no attempt is made
    // to prevent sending to a thread that may already be dead. If that's the case, silently ignore
    // the error, there is not much to do: any attempts at gracefully terminated a dead thread is a
    // dead end ;-).
    debug!("Broadcasting Abort message to all threads");
    let _res: Vec<_> = dispatchbox
        .keys()
        .filter(|chan| **chan != ChannelId::MainThread)
        .map(|chan| dispatchbox.send(chan, Message::ThreadAbort))
        .collect();
}

/// Main routine in charge of spawning and joining all threads.
pub fn run(max_drift_ppb: u32, phc_info: Option<PhcInfo>) {
    // Build a list of all channel ID (one per thread), and initialize all MPSC channels.
    let ids = vec![
        ChannelId::ClockErrorBoundPoller,
        ChannelId::MainThread,
        ChannelId::ShmWriter,
    ];
    let (mut mailbox, dispatchbox) = channels::new_channel_web::<ChannelId, Message>(ids);

    // Start all threads, keeping track of their respective handle to join them on.
    let mut thread_handlers = Vec::new();

    // Chrony poller.
    let mbox = match mailbox.get_mailbox(&ChannelId::ClockErrorBoundPoller) {
        Some(mbox) => mbox,
        None => unimplemented!(
            "Implementation error: no MPSC channel found for {:?}",
            ChannelId::ClockErrorBoundPoller
        ),
    };
    let ctx = Context {
        mbox,
        dbox: dispatchbox.clone(),
        channel_id: ChannelId::ClockErrorBoundPoller,
    };
    thread_handlers.push(spawn(|| chrony_poller::run(ctx, phc_info)));

    // Write to the shared memory segment.
    let mbox = match mailbox.get_mailbox(&ChannelId::ShmWriter) {
        Some(mbox) => mbox,
        None => unimplemented!(
            "Implementation error: no MPSC channel found for {:?}",
            ChannelId::ShmWriter
        ),
    };
    let ctx = Context {
        mbox,
        dbox: dispatchbox.clone(),
        channel_id: ChannelId::ShmWriter,
    };
    thread_handlers.push(spawn(move || shm_writer::run(ctx, max_drift_ppb)));

    // Listen for thread termination and panic messages.
    let mbox = match mailbox.get_mailbox(&ChannelId::MainThread) {
        Some(mbox) => mbox,
        None => unimplemented!(
            "Implementation error: no MPSC channel found for {:?}",
            ChannelId::MainThread
        ),
    };
    loop {
        match mbox.recv() {
            // A thread has stopped running ... for now, give it all up
            Ok(Message::ThreadTerminate(channel_id)) => {
                error!("Received terminate message from {:?}", channel_id);
                broadcast_abort(dispatchbox.clone());
                break;
            }
            // Got a panic message, tell everyone it is time to pick their marbles and go
            Ok(Message::ThreadPanic(channel_id)) => {
                error!("Received panic message from {:?}", channel_id);
                broadcast_abort(dispatchbox.clone());
                break;
            }
            Ok(_) => (),
            Err(e) => {
                error!("Lost communication with other threads, {:?}", e);
                broadcast_abort(dispatchbox.clone());
                break;
            }
        }
    }

    // Join all threads.
    for handle in thread_handlers {
        let _ = handle.join();
    }

    info!("ClockBound daemon is exiting");
}

#[cfg(test)]
mod t_thread_manager {

    use crate::channels::new_channel_web;

    use super::*;

    // Assert that all threads identified by ChannelId receive the Abort message. This test is
    // fairly contrived.
    #[test]
    fn test_broadcast_abort_to_all() {
        let channel_ids = vec![ChannelId::ShmWriter, ChannelId::ClockErrorBoundPoller];
        let (mut mbox, dbox) = new_channel_web(channel_ids.clone());

        broadcast_abort(dbox);

        assert!(channel_ids.iter().all(|chan| {
            let msg = mbox.get_mailbox(chan).unwrap().recv().unwrap();
            msg == Message::ThreadAbort
        }));
    }

    // Assert that Abort message is not sent to the main thread id
    // fairly contrived.
    #[test]
    fn test_broadcast_abort_do_not_send_to_main() {
        let channel_ids = vec![ChannelId::MainThread];
        let (mut mbox, dbox) = new_channel_web(channel_ids);

        broadcast_abort(dbox);

        assert!(mbox
            .get_mailbox(&ChannelId::MainThread)
            .unwrap()
            .recv()
            .is_err());
    }
}
