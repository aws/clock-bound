//! Receive stream
//!
//! This module implements the logic needed to retrieve `event::Event` objects from provisioned IO components
//! and return `RoutableEvent` objects.

use futures::StreamExt;
use futures::{
    Stream,
    stream::{SelectAll, once, select_all},
};
use rand::seq::SliceRandom;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use thiserror::Error;
use tracing::info;

use crate::daemon::async_ring_buffer::{BufferClosedError, Receiver};
use crate::daemon::event::{Event, Ntp};

#[derive(Debug, Error)]
pub enum ReceiverStreamError {
    #[error("Failed to initialize ReceiverStream.")]
    InitError(String),
}

/// Type to hold the stream produced from each `SourceIO` components receiver
type EventStream<'a> =
    Pin<Box<dyn Stream<Item = (SourceId, Result<Event, BufferClosedError>)> + 'a>>;

pub struct ReceiverStream {
    ntp_sources: HashMap<SourceId, Receiver<Ntp>>,
    link_local: Receiver<Ntp>,
}

#[bon::bon]
impl ReceiverStream {
    /// Initializer for `ReceiverStream` struct
    #[builder]
    pub fn new(
        link_local_receiver: Receiver<Ntp>,
        ntp_source_vec: Vec<(SocketAddr, Receiver<Ntp>)>,
    ) -> Self {
        let mut rs = ReceiverStream {
            ntp_sources: HashMap::new(),
            link_local: link_local_receiver,
        };
        for (source_ip, source_receiver) in ntp_source_vec {
            rs.add_ntp_source(source_ip, source_receiver);
        }

        rs
    }
}

/// `ReceiverStream` provides methods for aggregating the events delivered
///  to ring buffers associated to separate `SourceIO` time sources.
impl ReceiverStream {
    /// Adds a new ntp source to the `ntp_sources`
    fn add_ntp_source(&mut self, id: SocketAddr, receiver: Receiver<Ntp>) {
        let _ = &self.ntp_sources.insert(SourceId::NTPSource(id), receiver);
    }

    /// Creates an aggregated stream of results from all `SourceIO` component `Receivers`.
    fn get_aggregate_stream(&mut self) -> SelectAll<EventStream<'_>> {
        let mut streams: Vec<EventStream<'_>> = Vec::new();

        // Add NTP source streams
        for (source_id, source_receiver) in &mut self.ntp_sources {
            let source_id = *source_id;
            streams.push(Box::pin(
                once(source_receiver.recv()).map(move |result| (source_id, result.map(Event::Ntp))),
            ));
        }

        // Add the link_local receiver to the streams
        streams.push(Box::pin(
            once(self.link_local.recv())
                .map(|result| (SourceId::LinkLocal, result.map(Event::Ntp))),
        ));

        // Shuffles vector to avoid unfair treatment of any preloaded events.
        // Context: Without this shuffle, if events are loaded into the buffer before `recv` is called,
        // those events will be returned in the order that their relative stream is added to the `streams` vector.
        // Ex: In the "receiver_stream_test()" function the `RoutableEvent::LinkLocalEvent` will always be received second,
        // although it was sent by through it's relative buffer first)
        //
        // In the case that an actor produces events faster than we can poll,
        // not considering fairness when retrieving events opens the door for starvation.
        // In the future, we may implement more robust logic to consider fairness with event delivery.
        let mut rng = rand::rng();
        streams.shuffle(&mut rng);

        select_all(streams)
    }

    /// Creates an aggregate stream containing all results from `SourceIO` component `Receiver`s tracked by the struct
    ///
    /// # Returns
    /// a `RoutableEvent` wrapping the first event returned by the aggregate stream.
    pub async fn recv(&mut self) -> Option<RoutableEvent> {
        let mut result_stream = self.get_aggregate_stream();
        // Handle first result from the stream
        let Some((source_id, event_result)) = result_stream.next().await else {
            info!("Aggregate stream is empty, no futures to await");
            return None;
        };

        match event_result {
            Ok(Event::Ntp(ntp_event)) => match source_id {
                SourceId::LinkLocal => Some(RoutableEvent::LinkLocalEvent(ntp_event)),
                SourceId::NTPSource(id) => Some(RoutableEvent::NTPSourceEvent(id, ntp_event)),
            },
            Ok(Event::Phc) => {
                // FIXME: returned event should house data delivered from the PHC read
                todo!("Implement PHC IO source and data struct");
            }
            Err(_) => {
                todo!(
                    "Implement logic for buffers closing. We do not expect this to happen as a part of the alpha release implementation"
                );
            }
        }
    }
}

#[derive(Debug, Hash, PartialEq, std::cmp::Eq, Clone, Copy)]
enum SourceId {
    LinkLocal,
    NTPSource(SocketAddr),
}

#[derive(Debug, PartialEq)]
pub enum RoutableEvent {
    LinkLocalEvent(Ntp),
    NTPSourceEvent(SocketAddr, Ntp),
    // FIXME: The PhcEvent should wrap around phc data.
    // The phc data struct has yet to be implemented. Below enum is a placeholder
    PhcEvent,
}

#[tokio::test]
async fn receiver_stream_test() {
    use std::net::{IpAddr, Ipv4Addr};

    use crate::daemon::async_ring_buffer::create;
    use crate::daemon::event::{NtpData, Stratum};
    use crate::daemon::time::{Duration, Instant, TscCount};

    let (link_local_tx, link_local_rx) = create(1);

    let (ntp_source_tx, ntp_source_rx) = create(1);
    let dummy_ntp_source_ip = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 123);

    let mut rx_stream = ReceiverStream::builder()
        .link_local_receiver(link_local_rx)
        .ntp_source_vec(vec![(dummy_ntp_source_ip, ntp_source_rx)])
        .build();

    let dummy_ntp_data = Ntp::builder()
        .tsc_pre(TscCount::new(1))
        .tsc_post(TscCount::new(2))
        .ntp_data(NtpData {
            server_recv_time: Instant::new(1),
            server_send_time: Instant::new(2),
            root_delay: Duration::new(3),
            root_dispersion: Duration::new(4),
            stratum: Stratum::ONE,
        })
        .build()
        .unwrap();

    let _ = link_local_tx.send(dummy_ntp_data.clone());
    let _ = ntp_source_tx.send(dummy_ntp_data.clone());
    let num_events = 2;
    let mut counter = 0;

    for _ in 0..num_events {
        match rx_stream.recv().await.unwrap() {
            RoutableEvent::LinkLocalEvent(data) => {
                counter += 1;
                assert_eq!(
                    RoutableEvent::LinkLocalEvent(dummy_ntp_data.clone()),
                    RoutableEvent::LinkLocalEvent(data)
                );
            }
            RoutableEvent::NTPSourceEvent(ip, data) => {
                counter += 1;
                assert_eq!(
                    RoutableEvent::NTPSourceEvent(dummy_ntp_source_ip, dummy_ntp_data.clone()),
                    RoutableEvent::NTPSourceEvent(ip, data)
                );
            }
            RoutableEvent::PhcEvent => {
                assert!(false, "Phc event delivery has yet to be implemented")
            }
        };
    }
    assert!(
        counter.eq(&num_events),
        "{}",
        format!("{:#?} :: {:#?}", counter, num_events)
    );
}
