//! Receive stream
//!
//! This module implements the logic needed to retrieve `event::Event` objects from provisioned IO components
//! and return `RoutableEvent` objects.

#[cfg(feature = "daemon")]
use {
    super::io::ip_addr_source::Receiver as IpAddrSourceReceiver,
    crate::daemon::async_ring_buffer::{BufferClosedError, Receiver},
    futures::StreamExt,
    futures::{
        Stream,
        stream::{SelectAll, once, select_all},
    },
    rand::seq::SliceRandom,
    std::collections::HashMap,
    std::pin::Pin,
    thiserror::Error,
    tracing::info,
};

use std::net::SocketAddr;

use crate::daemon::clock_sync_algorithm::source::DevicePath;
use crate::daemon::event::{self, TscRtt};
use crate::daemon::time::TscCount;

#[cfg(feature = "daemon")]
#[derive(Debug, Error)]
pub enum ReceiverStreamError {
    #[error("Failed to initialize ReceiverStream.")]
    InitError(String),
}

/// Type to hold the stream produced from each `SourceIO` components receiver
#[cfg(feature = "daemon")]
type EventStream<'a> =
    Pin<Box<dyn Stream<Item = Result<RoutableEvent, BufferClosedError>> + 'a + Send>>;

#[cfg(feature = "daemon")]
#[derive(Debug, Default, bon::Builder)]
pub struct ReceiverStream {
    #[builder(default)]
    ntp_sources: HashMap<SocketAddr, Receiver<event::Ntp>>,
    amazon_time_sync: Option<Receiver<event::Ntp>>,
    phc: Option<(DevicePath, Receiver<event::Phc>)>,
}

/// `ReceiverStream` provides methods for aggregating the events delivered
///  to ring buffers associated to separate `SourceIO` time sources.
#[cfg(feature = "daemon")]
impl ReceiverStream {
    /// Adds a new ntp source to the `ntp_sources`
    pub fn add_ntp_source(&mut self, source: IpAddrSourceReceiver) {
        let (socket_addr, receiver) = source;
        let _ = &self.ntp_sources.insert(socket_addr, receiver);
    }

    /// Removes an ntp source from `ntp_sources`
    pub fn remove_ntp_source(&mut self, id: &SocketAddr) {
        self.ntp_sources.remove(id);
    }

    /// Set the amazon time sync source used by the receiver stream
    ///
    /// # Panics
    ///
    /// Panics if an Amazon Time Sync receiver has already been set.
    pub fn set_amazon_time_sync(&mut self, source: Receiver<event::Ntp>) {
        assert!(self.amazon_time_sync.is_none());
        self.amazon_time_sync = Some(source);
    }

    /// Amazon time sync getter
    pub fn amazon_time_sync(&self) -> Option<&Receiver<event::Ntp>> {
        self.amazon_time_sync.as_ref()
    }

    /// Set the phc source used by the receiver stream
    ///
    /// # Panics
    ///
    /// Panics if a PHC receiver has already been set.
    pub fn set_phc(&mut self, device_path: DevicePath, source: Receiver<event::Phc>) {
        assert!(self.phc.is_none());
        self.phc = Some((device_path, source));
    }

    /// Phc getter
    pub fn phc(&self) -> Option<&Receiver<event::Phc>> {
        self.phc.as_ref().map(|(_, receiver)| receiver)
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

        // Add the Amazon Time Sync receiver to the streams
        if let Some(amazon_time_sync) = &mut self.amazon_time_sync {
            streams.push(Box::pin(
                once(amazon_time_sync.recv())
                    .map(|result| result.map(RoutableEvent::AmazonTimeSync)),
            ));
        }

        // Add PHC if it's available
        if let Some((device_path, phc_receiver)) = &mut self.phc {
            streams.push(Box::pin(once(phc_receiver.recv()).map(move |result| {
                result.map(|event| RoutableEvent::Phc(device_path.clone(), event))
            })));
        }

        // Shuffles vector to avoid unfair treatment of any preloaded events.
        // Context: Without this shuffle, if events are loaded into the buffer before `recv` is called,
        // those events will be returned in the order that their relative stream is added to the `streams` vector.
        // Ex: In the "receiver_stream_test()" function the `RoutableEvent::AmazonTimeSync` will always be received second,
        // although it was sent by through it's relative buffer first)
        //
        // In the case that an actor produces events faster than we can poll,
        // not considering fairness when retrieving events opens the door for starvation.
        // In the future, we may implement more robust logic to consider fairness with event delivery.
        let mut rng = rand::rng();
        streams.shuffle(&mut rng);

        select_all(streams)
    }

    /// Number of configured receivers
    pub fn len(&self) -> usize {
        // destructure to ensure we catch new members at compile time
        let Self {
            ntp_sources,
            amazon_time_sync,
            phc,
        } = self;

        let mut retval = ntp_sources.len();
        if amazon_time_sync.is_some() {
            retval += 1;
        }

        if phc.is_some() {
            retval += 1;
        }

        retval
    }

    /// Returns `true` if no receivers are configured.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns `true` if an NTP source receiver is configured for `addr`.
    #[cfg(test)]
    pub(crate) fn contains_ntp_source(&self, addr: &SocketAddr) -> bool {
        self.ntp_sources.contains_key(addr)
    }

    /// Creates an aggregate stream containing all results from `SourceIO` component `Receiver`s tracked by the struct
    ///
    /// This will immediately return if there are no configured streams
    ///
    /// # Returns
    /// a `RoutableEvent` wrapping the first event returned by the aggregate stream.
    #[expect(clippy::missing_panics_doc, reason = "not expected in alpha")]
    pub async fn recv(&mut self) -> Option<RoutableEvent> {
        let mut result_stream = self.get_aggregate_stream();
        // Handle first result from the stream
        let Some(event_result) = result_stream.next().await else {
            info!("Aggregate stream is empty, no futures to await.");
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
            amazon_time_sync,
            ntp_sources,
            phc,
        } = self;

        if let Some(amazon_time_sync) = amazon_time_sync {
            amazon_time_sync.handle_disruption();
        }

        for source in ntp_sources.values_mut() {
            source.handle_disruption();
        }

        if let Some((_, phc)) = phc {
            phc.handle_disruption();
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum RoutableEvent {
    AmazonTimeSync(event::Ntp),
    NtpSource(SocketAddr, event::Ntp),
    Phc(DevicePath, event::Phc),
}

impl RoutableEvent {
    /// Get the system clock info
    #[cfg(not(test))]
    pub fn system_clock(&self) -> Option<&crate::daemon::event::SystemClockMeasurement> {
        match self {
            RoutableEvent::AmazonTimeSync(data) | RoutableEvent::NtpSource(_, data) => {
                data.system_clock()
            }
            RoutableEvent::Phc(_, data) => data.system_clock(),
        }
    }
}

impl TscRtt for RoutableEvent {
    fn counter_pre(&self) -> TscCount {
        match self {
            RoutableEvent::AmazonTimeSync(data) | RoutableEvent::NtpSource(_, data) => {
                data.counter_pre()
            }
            RoutableEvent::Phc(_, data) => data.counter_pre(),
        }
    }

    fn counter_post(&self) -> TscCount {
        match self {
            RoutableEvent::AmazonTimeSync(data) | RoutableEvent::NtpSource(_, data) => {
                data.counter_post()
            }
            RoutableEvent::Phc(_, data) => data.counter_post(),
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
        let (amazon_time_sync_tx, amazon_time_sync_rx) = create(1);

        let (ntp_source_tx, ntp_source_rx) = create(1);
        let dummy_ntp_source_ip = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 123);

        let mut rx_stream = ReceiverStream::builder()
            .amazon_time_sync(amazon_time_sync_rx)
            .ntp_sources(HashMap::from([(dummy_ntp_source_ip, ntp_source_rx)]))
            .build();

        let dummy_ntp_data = Ntp::builder()
            .counter_pre(TscCount::new(1))
            .counter_post(TscCount::new(2))
            .ntp_data(NtpData {
                server_recv_time: Instant::new(1),
                server_send_time: Instant::new(2),
                root_delay: Duration::new(3),
                root_dispersion: Duration::new(4),
                stratum: Stratum::ONE,
            })
            .build()
            .unwrap();

        amazon_time_sync_tx.send(dummy_ntp_data.clone()).unwrap();
        ntp_source_tx.send(dummy_ntp_data.clone()).unwrap();
        let num_events = 2;
        let mut counter = 0;

        for _ in 0..num_events {
            match rx_stream.recv().await.unwrap() {
                RoutableEvent::AmazonTimeSync(data) => {
                    counter += 1;
                    assert_eq!(
                        RoutableEvent::AmazonTimeSync(dummy_ntp_data.clone()),
                        RoutableEvent::AmazonTimeSync(data)
                    );
                }
                RoutableEvent::NtpSource(ip, data) => {
                    counter += 1;
                    assert_eq!(
                        RoutableEvent::NtpSource(dummy_ntp_source_ip, dummy_ntp_data.clone()),
                        RoutableEvent::NtpSource(ip, data)
                    );
                }
                RoutableEvent::Phc(..) => {
                    panic!("Phc event delivery has yet to be implemented")
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
        let (_amazon_time_sync_tx, amazon_time_sync_rx) = create(1);
        let (phc_tx, phc_rx) = create(1);

        let mut rx_stream = ReceiverStream::builder()
            .amazon_time_sync(amazon_time_sync_rx)
            .phc((DevicePath::from("/dev/ptp0"), phc_rx))
            .build();

        let phc_data = event::Phc::builder()
            .counter_pre(TscCount::new(1))
            .counter_post(TscCount::new(2))
            .data(PhcData {
                clock_error_bound: Duration::from_micros(20),
                time: Instant::from_days(3),
            })
            .build()
            .unwrap();

        phc_tx.send(phc_data.clone()).unwrap();

        let result = rx_stream.recv().await.unwrap();
        let RoutableEvent::Phc(_, data) = &result else {
            panic!("Expected to receive a Phc event, got {result:?}")
        };

        assert_eq!(*data, phc_data);
    }

    #[test]
    fn add_ntp_source() {
        let (_, amazon_time_sync_rx) = create(1);

        let (_, ntp_source_rx) = create(1);
        let dummy_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 123);
        let dummy_ntp_source_receiver: IpAddrSourceReceiver = (dummy_address, ntp_source_rx);

        let mut rx_stream = ReceiverStream::builder()
            .amazon_time_sync(amazon_time_sync_rx)
            .build();

        assert!(rx_stream.ntp_sources.is_empty());

        rx_stream.add_ntp_source(dummy_ntp_source_receiver);

        assert!(rx_stream.ntp_sources.len() == 1);
    }

    #[test]
    fn remove_ntp_source() {
        let (_, amazon_time_sync_rx) = create(1);
        let (_, ntp_source_rx) = create(1);

        let dummy_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 123);
        let dummy_ntp_source_receiver: IpAddrSourceReceiver = (dummy_address, ntp_source_rx);

        let mut rx_stream = ReceiverStream::builder()
            .amazon_time_sync(amazon_time_sync_rx)
            .ntp_sources(HashMap::from([dummy_ntp_source_receiver]))
            .build();

        assert!(rx_stream.ntp_sources.len() == 1);

        rx_stream.remove_ntp_source(&dummy_address);

        assert!(rx_stream.ntp_sources.is_empty());
    }

    #[test]
    fn len_empty() {
        let rx_stream = ReceiverStream::builder().build();
        assert_eq!(rx_stream.len(), 0);
    }

    #[test]
    fn is_empty_reflects_configured_receivers() {
        let empty_stream = ReceiverStream::builder().build();
        assert!(empty_stream.is_empty());

        let (_, amazon_time_sync_rx) = create(1);
        let non_empty_stream = ReceiverStream::builder()
            .amazon_time_sync(amazon_time_sync_rx)
            .build();
        assert!(!non_empty_stream.is_empty());
    }

    #[test]
    fn len_amazon_time_sync_only() {
        let (_, amazon_time_sync_rx) = create(1);

        let rx_stream = ReceiverStream::builder()
            .amazon_time_sync(amazon_time_sync_rx)
            .build();

        assert_eq!(rx_stream.len(), 1);
    }

    #[test]
    fn len_phc_only() {
        let (_, phc_rx) = create(1);

        let rx_stream = ReceiverStream::builder()
            .phc((DevicePath::from("/dev/ptp0"), phc_rx))
            .build();

        assert_eq!(rx_stream.len(), 1);
    }

    #[test]
    fn len_ntp_sources_only() {
        let (_, ntp_source_rx_a) = create(1);
        let (_, ntp_source_rx_b) = create(1);

        let addr_a = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 123);
        let addr_b = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), 123);

        let rx_stream = ReceiverStream::builder()
            .ntp_sources(HashMap::from([
                (addr_a, ntp_source_rx_a),
                (addr_b, ntp_source_rx_b),
            ]))
            .build();

        assert_eq!(rx_stream.len(), 2);
    }

    #[test]
    fn len_all_sources() {
        let (_, amazon_time_sync_rx) = create(1);
        let (_, phc_rx) = create(1);
        let (_, ntp_source_rx) = create(1);

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 123);

        let rx_stream = ReceiverStream::builder()
            .amazon_time_sync(amazon_time_sync_rx)
            .phc((DevicePath::from("/dev/ptp0"), phc_rx))
            .ntp_sources(HashMap::from([(addr, ntp_source_rx)]))
            .build();

        // 1 amazon time sync + 1 phc + 1 ntp source
        assert_eq!(rx_stream.len(), 3);
    }

    #[test]
    fn len_reflects_add_and_remove_ntp_source() {
        let (_, amazon_time_sync_rx) = create(1);
        let (_, ntp_source_rx) = create(1);

        let dummy_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 123);
        let dummy_ntp_source_receiver: IpAddrSourceReceiver = (dummy_address, ntp_source_rx);

        let mut rx_stream = ReceiverStream::builder()
            .amazon_time_sync(amazon_time_sync_rx)
            .build();

        assert_eq!(rx_stream.len(), 1);

        rx_stream.add_ntp_source(dummy_ntp_source_receiver);
        assert_eq!(rx_stream.len(), 2);

        rx_stream.remove_ntp_source(&dummy_address);
        assert_eq!(rx_stream.len(), 1);
    }
    #[tokio::test]
    async fn pool_source_delivered_as_ntp_source() {
        let (_amazon_time_sync_tx, amazon_time_sync_rx) = create(1);
        let (pool_source_tx, pool_source_rx) = create(1);

        let source_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 123);

        // Pool sources are just NTP sources in the receiver stream
        let mut rx_stream = ReceiverStream::builder()
            .amazon_time_sync(amazon_time_sync_rx)
            .ntp_sources(HashMap::from([(source_addr, pool_source_rx)]))
            .build();

        let dummy_ntp_data = Ntp::builder()
            .counter_pre(TscCount::new(1))
            .counter_post(TscCount::new(2))
            .ntp_data(NtpData {
                server_recv_time: Instant::new(1),
                server_send_time: Instant::new(2),
                root_delay: Duration::new(3),
                root_dispersion: Duration::new(4),
                stratum: Stratum::ONE,
            })
            .build()
            .unwrap();

        pool_source_tx.send(dummy_ntp_data.clone()).unwrap();

        let result = rx_stream.recv().await.unwrap();
        let RoutableEvent::NtpSource(addr, data) = result else {
            panic!("Expected NtpSource event, got {result:?}");
        };
        assert_eq!(addr, source_addr);
        assert_eq!(data, dummy_ntp_data);
    }
}
