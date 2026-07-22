//! Clock Synchronization Daemon

pub mod clock_parameters;

pub mod clock_sync_algorithm;

pub mod time;

pub mod event;

pub mod receiver_stream;

pub mod selected_clock;

#[cfg(feature = "daemon")]
pub mod async_ring_buffer;

#[cfg(feature = "daemon")]
pub mod io;

#[cfg(feature = "daemon")]
pub mod clock_state;

pub mod subscriber;

// --- Everything below is only needed for the full daemon ---

#[cfg(feature = "daemon")]
use {
    crate::daemon::{
        async_ring_buffer::Sender,
        clock_parameters::ClockParameters,
        clock_state::ClockState,
        clock_sync_algorithm::{ClockSyncAlgorithm, Selector, source::NtpSource},
        io::{
            ClockDisruptionEvent,
            ntp::{DaemonInfo, NTPSourceReceiver, NTPSourceSender},
        },
        receiver_stream::{ReceiverStream, RoutableEvent},
        selected_clock::SelectedClockSource,
        time::tsc::Skew,
    },
    crate::vmclock::shm::VMCLOCK_SHM_DEFAULT_PATH,
    rand::{RngCore, rng},
    std::sync::Arc,
    tokio::sync::watch,
    tokio_util::{sync::CancellationToken, task::TaskTracker},
    tracing::{error, info},
};

/// The maximum dispersion growth every second
///
/// In between updates, the clock error bound continuously grows to take into account the worse
/// case drift of the underlying oscillator. The maximum dispersion is the rate of growth applied
/// to the last clock error bound update.
///
/// If the value is 15,000 parts per billion, for example, then every second we go without an
/// updated measurement the clock error bound will increase by 15 microseconds.
///
/// This number is based on CPU spec sheet error tolerances
#[cfg(feature = "daemon")]
pub(crate) const MAX_DISPERSION_GROWTH_PPB: u32 = 15_000;

#[cfg(feature = "daemon")]
const MAX_DISPERSION_GROWTH: Skew = Skew::from_ppb(MAX_DISPERSION_GROWTH_PPB as f64);

#[cfg(feature = "daemon")]
pub struct Daemon {
    io_front_end: io::SourceIO,
    clock_sync_algorithm: ClockSyncAlgorithm,
    receiver_stream: ReceiverStream,
    clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
    cancellation_token: CancellationToken,
    clock_state_handle: ClockStateHandle,
}

#[cfg(feature = "daemon")]
impl Daemon {
    /// Construct and initialize a new daemon
    /// FIXME: Make this function not async. (Currently required for the io.run methods)
    pub async fn construct(cancellation_token: CancellationToken) -> Self {
        let daemon_info = DaemonInfo {
            major_version: 2,
            minor_version: 100,
            startup_id: rng().next_u64(),
        };

        let clock_state_cancellation_token = CancellationToken::new();

        let selected_clock = Arc::new(SelectedClockSource::default());

        // Initialize IO components.
        let mut io_front_end = io::SourceIO::construct(selected_clock.clone(), daemon_info);
        let clock_disruption_receiver = io_front_end.clock_disruption_receiver();

        // Initialize link-local event buffer and IO component.
        let (link_local_tx, link_local_rx) = async_ring_buffer::create(2);
        io_front_end.create_link_local(link_local_tx).await;

        // Initialize ntp source event buffer and IO component.
        let ntp_sources =
            clock_sync_algorithm::source::NtpSource::create_time_aws_sources(MAX_DISPERSION_GROWTH);
        let (ntp_source_event_senders, ntp_source_event_receivers) =
            Self::init_ntp_source_buffers(2, &ntp_sources);
        for source in ntp_source_event_senders {
            io_front_end.create_ntp_source(source).await;
        }

        // Initialize vmclock IO component.
        io_front_end.create_vmclock(VMCLOCK_SHM_DEFAULT_PATH).await;
        #[expect(clippy::redundant_closure_for_method_calls)]
        let disruption_marker = io_front_end
            .vmclock()
            .map(|vmclock| vmclock.last_disruption_marker())
            .unwrap_or_default();
        let clock_disruption_support_enabled = io_front_end.vmclock().is_some();

        // Initialize PHC event buffer and IO component.
        let (phc_tx, phc_rx) = async_ring_buffer::create(2);
        io_front_end.create_phc(phc_tx).await;

        // Note: Failure to create a PHC IO component here is considered non-fatal,
        // we will continue without using the device as an clock sync input.
        let phc = io_front_end.phc().map(|io_phc| {
            clock_sync_algorithm::source::Phc::new(
                io_phc.device_path().to_owned(),
                MAX_DISPERSION_GROWTH,
            )
        });
        let phc_rx = if io_front_end.phc_exists() {
            Some(phc_rx)
        } else {
            None
        };

        // Initialize clock sync algorithm.
        let clock_sync_algorithm = ClockSyncAlgorithm::builder()
            .link_local(clock_sync_algorithm::source::LinkLocal::new(
                MAX_DISPERSION_GROWTH,
            ))
            .ntp_sources(ntp_sources)
            .maybe_phc(phc)
            .selected_clock(selected_clock.clone())
            .selector(Selector::new(MAX_DISPERSION_GROWTH))
            .build();

        // Initializing receiver stream with IO ring buffer receivers
        let receiver_stream: ReceiverStream = ReceiverStream::builder()
            .link_local(link_local_rx)
            .ntp_sources(ntp_source_event_receivers.into_iter().collect())
            .maybe_phc(phc_rx)
            .build();

        // Initialize the Clock State component
        let (clock_state_tx, clock_state) = {
            let (tx, rx) = async_ring_buffer::create(1);
            let clock_state = ClockState::construct(
                rx,
                clock_disruption_receiver.clone(),
                clock_state_cancellation_token.clone(),
                disruption_marker,
                clock_disruption_support_enabled,
            );
            (tx, clock_state)
        };
        let task_tracker = TaskTracker::new();
        let clock_state_handle = ClockStateHandle {
            clock_state: Some(clock_state),
            tx: clock_state_tx,
            cancellation_token: clock_state_cancellation_token,
            task_tracker,
        };

        Self {
            io_front_end,
            clock_sync_algorithm,
            receiver_stream,
            clock_disruption_receiver,
            cancellation_token,
            clock_state_handle,
        }
    }

    /// Run the daemon
    pub async fn run(mut self: Box<Self>) {
        self.clock_sync_algorithm.init_repro();
        // Start IO polling
        self.io_front_end.spawn_all();
        self.clock_state_handle.task_tracker.spawn({
            #[expect(
                clippy::missing_panics_doc,
                reason = "struct always initialized with Some"
            )]
            let mut clock_state = self.clock_state_handle.clock_state.take().unwrap();
            async move {
                clock_state.run().await;
            }
        });
        self.clock_state_handle.task_tracker.close();
        loop {
            tokio::select! {
                biased; // biased to ensure disruption is handled first when this happens
                Ok(()) = self.clock_disruption_receiver.changed() => {
                    self.handle_disruption();
                }
                Some(routable_event) = self.receiver_stream.recv() => {
                    self.handle_event(routable_event);
                }
                () = self.cancellation_token.cancelled() => {
                    info!("Received shutdown signal. Starting graceful shutdown of daemon.");

                    // TODO: we can asynchronously shutdown both clock state and io tasks.

                    // shutdown clock state task
                    self.clock_state_handle.cancellation_token.cancel();
                    self.clock_state_handle.task_tracker.wait().await;

                    // shutdown all io tasks
                    self.io_front_end.shutdown_all().await;

                    // exit ourselves
                    break;
                }
            }
        }
    }

    fn handle_event(&mut self, routable_event: RoutableEvent) {
        if let Some(params) = self.clock_sync_algorithm.feed(routable_event) {
            use crate::daemon::async_ring_buffer::SendError;

            match self.clock_state_handle.tx.send(params.clone()) {
                Ok(()) => (),
                Err(SendError::Disrupted(clock_parameters)) => {
                    // don't handle_disruption. It will be handled on the next call of tokio::select
                    info!(
                        ?clock_parameters,
                        "Trying to send a value when there was a disruption event. dropping."
                    );
                }
                Err(SendError::BufferClosed(e)) => {
                    error!(
                        ?e,
                        "Trying to send a value when the buffer is closed. Panicking."
                    );
                    panic!("Unable to communicate with clock state. {e:?}");
                }
            }
        }
    }

    /// Handle a clock disruption event
    fn handle_disruption(&mut self) {
        let Self {
            io_front_end: _,
            clock_sync_algorithm,
            receiver_stream,
            clock_disruption_receiver,
            cancellation_token: _,
            clock_state_handle,
        } = self;

        let ClockStateHandle {
            clock_state: _,
            tx,
            cancellation_token: _,
            task_tracker: _,
        } = clock_state_handle;

        let val = clock_disruption_receiver.borrow_and_update().clone();
        if val.disruption_marker.is_some() {
            tx.handle_disruption();
            clock_sync_algorithm.handle_disruption();
            receiver_stream.handle_disruption();
        }
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
        ntp_source_vec: &Vec<NtpSource>,
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

#[cfg(feature = "daemon")]
struct ClockStateHandle {
    clock_state: Option<ClockState>,
    tx: Sender<ClockParameters>,
    cancellation_token: CancellationToken,
    task_tracker: TaskTracker,
}
