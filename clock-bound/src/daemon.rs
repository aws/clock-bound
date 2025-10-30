//! Clock Synchronization Daemon

pub mod async_ring_buffer;

pub mod io;

pub mod clock_parameters;

pub mod clock_state;

pub mod clock_sync_algorithm;

pub mod time;

pub mod event;

pub mod receiver_stream;

use crate::daemon::{clock_sync_algorithm::ClockSyncAlgorithm, time::tsc::Skew};

/// The maximum dispersion growth every second
///
/// Whenever a clock error bound measurement is made, that value increases by this value
/// for every second that the measurement becomes stale.
///
/// If the value is 15,000 parts per billion, for example, then every second we go without an
/// updated measurement the clock error bound will increase by 15 microseconds.
///
/// This number is based on CPU spec sheet error tolerances
const MAX_DISPERSION_GROWTH_PBB: u32 = 15_000;

const MAX_DISPERSION_GROWTH: Skew = Skew::from_ppb(MAX_DISPERSION_GROWTH_PBB as f64);

pub struct Daemon {
    _io_front_end: io::SourceIO,
    clock_sync_algorithm: ClockSyncAlgorithm,
    link_local_receiver: async_ring_buffer::Receiver<event::Ntp>,
}

impl Daemon {
    /// Construct and initialize a new daemon
    /// FIXME: Make this function not async. (Currently required for the io.run methods)
    pub async fn construct() -> Self {
        let (tx, rx) = async_ring_buffer::create(2);

        let mut io_front_end = io::SourceIO::construct();

        let clock_sync_algorithm = ClockSyncAlgorithm::builder()
            .link_local(clock_sync_algorithm::source::LinkLocal::new(
                MAX_DISPERSION_GROWTH,
            ))
            .build();

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
        io_front_end.create_link_local(tx).await;
        // TODO: Refactor so that the construction and spawning occurs in separate functions.
        io_front_end.spawn_all();

        Self {
            _io_front_end: io_front_end,
            clock_sync_algorithm,
            link_local_receiver: rx,
        }
    }

    /// Run the daemon
    pub async fn run(&mut self) {
        loop {
            // TODO: add live migration watch and statements
            tokio::select! {
                Ok(event) = self.link_local_receiver.recv() => {
                    self.handle_event(event);
                }
            }
        }
    }

    fn handle_event(&mut self, event: event::Ntp) {
        self.clock_sync_algorithm.feed_link_local(event);
    }
}
