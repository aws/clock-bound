//! Clock Synchronization Daemon

pub mod async_ring_buffer;

pub mod io;

pub mod clock_parameters;

pub mod clock_state;

pub mod clock_sync_algorithm;

pub mod time;

pub mod event;

pub mod receiver_stream;

pub mod subscriber;

pub mod selected_clock;

use std::sync::Arc;

use crate::daemon::{
    clock_sync_algorithm::{ClockSyncAlgorithm, Selector, source::NTPSource},
    io::ntp::{DaemonInfo, NTPSourceReceiver, NTPSourceSender},
    receiver_stream::{ReceiverStream, RoutableEvent},
    selected_clock::SelectedClockSource,
    time::tsc::Skew,
};

#[cfg(not(feature = "test-side-by-side"))]
use crate::daemon::{
    async_ring_buffer::Sender, clock_parameters::ClockParameters, clock_state::ClockState,
};
use rand::{RngCore, rng};

/// The maximum dispersion growth every second
///
/// Whenever a clock error bound measurement is made, that value increases by this value
/// for every second that the measurement becomes stale.
///
/// If the value is 15,000 parts per billion, for example, then every second we go without an
/// updated measurement the clock error bound will increase by 15 microseconds.
///
/// This number is based on CPU spec sheet error tolerances
pub(crate) const MAX_DISPERSION_GROWTH_PPB: u32 = 15_000;

const MAX_DISPERSION_GROWTH: Skew = Skew::from_ppb(MAX_DISPERSION_GROWTH_PPB as f64);

pub struct Daemon {
    _io_front_end: io::SourceIO,
    clock_sync_algorithm: ClockSyncAlgorithm,
    receiver_stream: ReceiverStream,
    #[cfg(not(feature = "test-side-by-side"))]
    clock_state_tx: Sender<ClockParameters>,
}

impl Daemon {
    /// Construct and initialize a new daemon
    /// FIXME: Make this function not async. (Currently required for the io.run methods)
    pub async fn construct() -> Self {
        let daemon_info = DaemonInfo {
            major_version: 2,
            minor_version: 100,
            startup_id: rng().next_u64(),
        };
        #[cfg(not(feature = "test-side-by-side"))]
        let (clock_state_tx, mut clock_state) = {
            let (tx, rx) = async_ring_buffer::create(1);
            let clock_state = ClockState::construct(rx);
            (tx, clock_state)
        };

        let selected_clock = Arc::new(SelectedClockSource::default());
        let clock_sync_algorithm = ClockSyncAlgorithm::builder()
            .link_local(clock_sync_algorithm::source::LinkLocal::new(
                MAX_DISPERSION_GROWTH,
            ))
            .ntp_sources(
                clock_sync_algorithm::source::NTPSource::create_time_aws_sources(
                    MAX_DISPERSION_GROWTH,
                ),
            )
            .selected_clock(selected_clock.clone())
            .selector(Selector::new(MAX_DISPERSION_GROWTH))
            .build();

        // Initializing async ring buffers for IO event delivery
        let (link_local_tx, link_local_rx) = async_ring_buffer::create(2);
        let (ntp_source_event_senders, ntp_source_event_receivers) =
            Self::init_ntp_source_buffers(2, &clock_sync_algorithm.ntp_sources);

        // Initializing receiver stream with IO ring buffer receivers
        let receiver_stream: ReceiverStream = ReceiverStream::builder()
            .link_local(link_local_rx)
            .ntp_sources(ntp_source_event_receivers.into_iter().collect())
            .build();

        let mut io_front_end = io::SourceIO::construct(selected_clock.clone(), daemon_info);
        // FIXME, we are basically starting the application in the constructor
        // We should be able to construct the link local and spawn it when `run` is called
        //
        // Also, this function doesn't return anything?
        //
        // Ideally we would construct the `io::LinkLocal` and just build the `io_front_end` with something like
        // ```
        // let (tx, rx) = mpsc::channel(1);
        // let link_local = io::LinkLocal::construct(rx, ..Args);
        // let io_front_end = SourceIo::builder().link_local(tx, link_local).build();
        // ```
        io_front_end.create_link_local(link_local_tx).await;
        for source in ntp_source_event_senders {
            io_front_end.create_ntp_source(source).await;
        }

        // Start IO polling
        io_front_end.spawn_all();
        #[cfg(not(feature = "test-side-by-side"))]
        tokio::spawn(async move {
            clock_state.run().await;
        });
        Self {
            _io_front_end: io_front_end,
            clock_sync_algorithm,
            receiver_stream,
            #[cfg(not(feature = "test-side-by-side"))]
            clock_state_tx,
        }
    }

    /// Run the daemon
    pub async fn run(&mut self) {
        loop {
            // TODO: add live migration watch and statements
            tokio::select! {
                Some(routable_event) = self.receiver_stream.recv() => {
                    self.handle_event(routable_event);
                }
            }
        }
    }

    fn handle_event(&mut self, routable_event: RoutableEvent) {
        #[cfg(not(feature = "test-side-by-side"))]
        if let Some(params) = self.clock_sync_algorithm.feed(routable_event) {
            self.clock_state_tx.send(params.clone()).unwrap();
        }
        #[cfg(feature = "test-side-by-side")]
        let _ = self.clock_sync_algorithm.feed(routable_event);
    }

    /// Takes in a vector of `source::NTPSource` structs and returns the `async_ring_buffer` senders and receivers
    /// for each `ntp_source`'s IO event delivery.
    /// # Parameters
    /// - `ntp_source_buffer_size`: The size of ring buffer to create for each source
    /// - `ntp_source_vec`: a pointer to a vector of `source::NTPSource`
    ///
    /// # Returns
    /// - A tuple containing two lists:
    /// > 1. `sender_vec`: Vector of `NTPSourceSender`
    /// > 2. `receiver_vec`: Vector of `NTPSourceReceiver`
    fn init_ntp_source_buffers(
        ntp_source_buffer_size: usize,
        ntp_source_vec: &Vec<NTPSource>,
    ) -> (Vec<NTPSourceSender>, Vec<NTPSourceReceiver>) {
        let mut sender_vec: Vec<NTPSourceSender> = Vec::new();
        let mut receiver_vec: Vec<NTPSourceReceiver> = Vec::new();

        for source in ntp_source_vec {
            let (tx, rx) = async_ring_buffer::create(ntp_source_buffer_size);
            sender_vec.push((source.socket_address(), tx));
            receiver_vec.push((source.socket_address(), rx));
        }

        (sender_vec, receiver_vec)
    }
}
