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
use crate::daemon::event::{self, TscRtt};
use crate::daemon::time::TscCount;

use super::io::ntp::NTPSourceReceiver;

#[derive(Debug, Error)]
pub enum ReceiverStreamError {
    #[error("Failed to initialize ReceiverStream.")]
    InitError(String),
}

/// Type to hold the stream produced from each `SourceIO` components receiver
type EventStream<'a> =
    Pin<Box<dyn Stream<Item = Result<RoutableEvent, BufferClosedError>> + 'a + Send>>;

#[derive(Debug, bon::Builder)]
pub struct ReceiverStream {
    #[builder(default)]
    ntp_sources: HashMap<SocketAddr, Receiver<event::Ntp>>,
    link_local: Receiver<event::Ntp>,
    phc: Option<Receiver<event::Phc>>,
}

/// `ReceiverStream` provides methods for aggregating the events delivered
///  to ring buffers associated to separate `SourceIO` time sources.
impl ReceiverStream {
    /// Adds a new ntp source to the `ntp_sources`
    pub fn add_ntp_source(&mut self, source: NTPSourceReceiver) {
        let (socket_addr, receiver) = source;
        let _ = &self.ntp_sources.insert(socket_addr, receiver);
    }

    /// Removes an ntp source from `ntp_sources`
    pub fn remove_ntp_source(&mut self, id: &SocketAddr) {
        if self.ntp_sources.contains_key(id) {
            self.ntp_sources.remove(id);
        }
    }

    /// Creates an aggregated stream of results from all `SourceIO` component `Receivers`.
    fn get_aggregate_stream(&mut self) -> SelectAll<EventStream<'_>> {
        let mut streams: Vec<EventStream<'_>> = Vec::new();

        // Add NTP source streams
        for (source_id, source_receiver) in &mut self.ntp_sources {
            let source_id = *source_id;
            streams.push(Box::pin(once(source_receiver.recv()).map(move |result| {
                result.map(|event| RoutableEvent::NtpSource(source_id, event))
            })));
        }

        // Add the link_local receiver to the streams
        streams.push(Box::pin(
            once(self.link_local.recv()).map(|result| result.map(RoutableEvent::LinkLocal)),
        ));

        // Add PHC if it's available
        if let Some(phc_receiver) = &mut self.phc {
            streams.push(Box::pin(
                once(phc_receiver.recv()).map(|result| result.map(RoutableEvent::Phc)),
            ));
        }

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
    #[expect(clippy::missing_panics_doc, reason = "not expected in alpha")]
    pub async fn recv(&mut self) -> Option<RoutableEvent> {
        let mut result_stream = self.get_aggregate_stream();
        // Handle first result from the stream
        let Some(event_result) = result_stream.next().await else {
            info!("Aggregate stream is empty, no futures to await");
            return None;
        };

        let routable_event = event_result.expect("todo: Implement logic for buffers closing. We do not expect this to happen as a part of the alpha release implementation");
        Some(routable_event)
    }

    /// Handle a clock disruption event
    ///
    /// This struct should loop through all receivers and clear the buffers when possible
    pub fn handle_disruption(&mut self) {
        let Self {
            link_local,
            ntp_sources,
            phc,
        } = self;

        link_local.handle_disruption();
        for source in ntp_sources.values_mut() {
            source.handle_disruption();
        }

        if let Some(phc) = phc {
            phc.handle_disruption();
        }
    }
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub enum RoutableEvent {
    LinkLocal(event::Ntp),
    NtpSource(SocketAddr, event::Ntp),
    Phc(event::Phc),
}

impl RoutableEvent {
    /// Get the system clock info
    #[cfg(not(test))]
    pub fn system_clock(&self) -> Option<&crate::daemon::event::SystemClockMeasurement> {
        match self {
            RoutableEvent::LinkLocal(data) | RoutableEvent::NtpSource(_, data) => {
                data.system_clock()
            }
            RoutableEvent::Phc(data) => data.system_clock(),
        }
    }
}

impl TscRtt for RoutableEvent {
    fn tsc_pre(&self) -> TscCount {
        match self {
            RoutableEvent::LinkLocal(data) | RoutableEvent::NtpSource(_, data) => data.tsc_pre(),
            RoutableEvent::Phc(data) => data.tsc_pre(),
        }
    }

    fn tsc_post(&self) -> TscCount {
        match self {
            RoutableEvent::LinkLocal(data) | RoutableEvent::NtpSource(_, data) => data.tsc_post(),
            RoutableEvent::Phc(data) => data.tsc_post(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use super::*;
    use crate::daemon::async_ring_buffer::create;
    use crate::daemon::event::{Ntp, NtpData, PhcData, Stratum};
    use crate::daemon::time::{Duration, Instant, TscCount};

    #[tokio::test]
    async fn receiver_stream() {
        let (link_local_tx, link_local_rx) = create(1);

        let (ntp_source_tx, ntp_source_rx) = create(1);
        let dummy_ntp_source_ip = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 123);

        let mut rx_stream = ReceiverStream::builder()
            .link_local(link_local_rx)
            .ntp_sources(HashMap::from([(dummy_ntp_source_ip, ntp_source_rx)]))
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

        link_local_tx.send(dummy_ntp_data.clone()).unwrap();
        ntp_source_tx.send(dummy_ntp_data.clone()).unwrap();
        let num_events = 2;
        let mut counter = 0;

        for _ in 0..num_events {
            match rx_stream.recv().await.unwrap() {
                RoutableEvent::LinkLocal(data) => {
                    counter += 1;
                    assert_eq!(
                        RoutableEvent::LinkLocal(dummy_ntp_data.clone()),
                        RoutableEvent::LinkLocal(data)
                    );
                }
                RoutableEvent::NtpSource(ip, data) => {
                    counter += 1;
                    assert_eq!(
                        RoutableEvent::NtpSource(dummy_ntp_source_ip, dummy_ntp_data.clone()),
                        RoutableEvent::NtpSource(ip, data)
                    );
                }
                RoutableEvent::Phc(_data) => {
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

    #[tokio::test]
    async fn phc_stream() {
        let (_link_local_tx, link_local_rx) = create(1);
        let (phc_tx, phc_rx) = create(1);

        let mut rx_stream = ReceiverStream::builder()
            .link_local(link_local_rx)
            .phc(phc_rx)
            .build();

        let phc_data = event::Phc::builder()
            .tsc_pre(TscCount::new(1))
            .tsc_post(TscCount::new(2))
            .data(PhcData {
                clock_error_bound: Duration::from_micros(20),
                time: Instant::from_days(3),
            })
            .build()
            .unwrap();

        phc_tx.send(phc_data.clone()).unwrap();

        let result = rx_stream.recv().await.unwrap();
        let RoutableEvent::Phc(data) = &result else {
            panic!("Expected to receive a Phc event, got {result:?}")
        };

        assert_eq!(*data, phc_data);
    }

    #[test]
    fn add_ntp_source() {
        let (_, link_local_rx) = create(1);

        let (_, ntp_source_rx) = create(1);
        let dummy_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 123);
        let dummy_ntp_source_receiver: NTPSourceReceiver = (dummy_address, ntp_source_rx);

        let mut rx_stream = ReceiverStream::builder().link_local(link_local_rx).build();

        assert!(rx_stream.ntp_sources.is_empty());

        rx_stream.add_ntp_source(dummy_ntp_source_receiver);

        assert!(rx_stream.ntp_sources.len() == 1);
    }

    #[test]
    fn remove_ntp_source() {
        let (_, link_local_rx) = create(1);
        let (_, ntp_source_rx) = create(1);

        let dummy_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 123);
        let dummy_ntp_source_receiver: NTPSourceReceiver = (dummy_address, ntp_source_rx);

        let mut rx_stream = ReceiverStream::builder()
            .link_local(link_local_rx)
            .ntp_sources(HashMap::from([dummy_ntp_source_receiver]))
            .build();

        assert!(rx_stream.ntp_sources.len() == 1);

        rx_stream.remove_ntp_source(&dummy_address);

        assert!(rx_stream.ntp_sources.is_empty());
    }
}
