//! Perform IO on clock events
//!
//! This module implements the logic needed to retrieve time sync sample measurements, be it from NTP sources from
//! over the internet, the PHC via Linux's `ioctl` interface or some other source.

#![allow(dead_code)]

use std::collections::HashMap;

use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};
use tokio::task::spawn;
use tracing::{debug, info};

pub mod ntp;
use super::event;
use ntp::LinkLocal;

mod tsc;

/// `SourceIO` acts as the front end for IO tasks.
///
/// `SourceIO` contains the interface from which new IO tasks can be spawned, as well as an interface
/// to send control commands to the specific IO tasks.
pub struct SourceIO {
    /// A mapping between the time source and the control sender.
    sources: HashMap<TimeSource, mpsc::Sender<ControlRequest>>,
    /// Contains the channel used to communicate clock disruption events.
    clock_disruption_channels: ClockDisruptionChannels,
}

impl SourceIO {
    /// Constructs a new `SourceIO` object and constructs the necessary resources.
    pub fn construct() -> Self {
        let (sender, receiver) = watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent {});
        SourceIO {
            sources: HashMap::new(),
            clock_disruption_channels: ClockDisruptionChannels { sender, receiver },
        }
    }

    /// Spawns the IO task for sampling the Link Local NTP source.
    ///
    /// # Panics
    /// - If not called within the `tokio` runtime.
    /// - If socket binding fails.
    pub fn create_link_local(&mut self, event_sender: mpsc::Sender<event::Ntp>) {
        info!("Creating link local source.");

        let entry = self.sources.entry(TimeSource::LinkLocal);
        debug!(?entry, "Current source entry status");
        entry.or_insert_with(|| {
            let (ctrl_sender, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
            let clock_disruption_receiver = self.clock_disruption_channels.sender.subscribe();

            spawn(async move {
                let socket = UdpSocket::bind(ntp::UNSPECIFIED_SOCKET_ADDRESS)
                    .await
                    .unwrap();
                let mut link_local = LinkLocal::construct(
                    socket,
                    event_sender,
                    ctrl_receiver,
                    clock_disruption_receiver,
                );
                link_local.run().await;
            });
            ctrl_sender
        });

        info!("Source update complete.");
    }

    /// Starts the control flow task.
    #[allow(
        clippy::unused_async,
        reason = "This is a stubbed function. The async component will be implemented at a later date."
    )]
    pub async fn run(&self) {}
}

/// Communication channels for sending and receiving clock disruption events.
struct ClockDisruptionChannels {
    sender: watch::Sender<ClockDisruptionEvent>,
    receiver: watch::Receiver<ClockDisruptionEvent>,
}

// TODO: This is a stub for future clock disruption events.
#[derive(Clone, Debug)]
pub struct ClockDisruptionEvent {}

// TODO: This is a stub for future control events.
#[derive(Debug)]
pub struct ControlRequest {}

/// `TimeSource` is a type representing the possible time sources the daemon can collect samples
/// from.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum TimeSource {
    /// The internal AWS EC2 link local source `169.254.169.123`.
    LinkLocal,
}
