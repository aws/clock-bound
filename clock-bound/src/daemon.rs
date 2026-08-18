//! Clock Synchronization Daemon

#[cfg(feature = "algorithm")]
pub mod algo_analysis;

pub mod clock_parameters;

pub mod clock_sync_algorithm;

pub mod time;

pub mod event;

pub mod receiver_stream;

pub mod selected_clock;

#[cfg(feature = "daemon")]
pub mod autodetect;

#[cfg(feature = "daemon")]
pub mod source_mutator;

#[cfg(feature = "daemon")]
pub mod async_ring_buffer;

#[cfg(feature = "daemon")]
pub mod io;

#[cfg(feature = "daemon")]
pub mod clock_state;

#[cfg(feature = "daemon")]
pub mod config;

pub mod logging;

#[cfg(feature = "daemon")]
pub mod message;

// --- Everything below is only needed for the full daemon ---

#[cfg(feature = "daemon")]
use {
    crate::{
        daemon::{
            async_ring_buffer::Sender,
            autodetect::Autodetect,
            clock_state::ClockState,
            clock_sync_algorithm::{ClockSyncAlgorithm, Selector, SyncParameters},
            io::{ClockDisruptionEvent, ntp::DaemonInfo},
            message::Dns as DnsMessage,
            receiver_stream::{ReceiverStream, RoutableEvent},
            selected_clock::SelectedClockSource,
            source_mutator::SourceMutator,
            time::tsc::Skew,
        },
        shm::ClockStatus,
    },
    rand::{RngCore, rng},
    std::{
        net::{IpAddr, Ipv4Addr, SocketAddr},
        sync::Arc,
    },
    tokio::sync::{mpsc, watch},
    tokio_util::{sync::CancellationToken, task::TaskTracker},
    tracing::{error, info, warn},
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

/// AWS public time service addresses (time.aws.com)
#[cfg(feature = "daemon")]
pub const AWS_TEMP_PUBLIC_TIME_ADDRESSES: [SocketAddr; 2] = [
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(166, 117, 111, 42)), 123),
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(3, 33, 186, 244)), 123),
];

/// The number of messages that can be in the daemon's mpsc channel
///
/// There is no technical reason why this cannot be one. However, adding
/// more allows for less contention when initializing pools
#[cfg(feature = "daemon")]
const DAEMON_MESSAGE_CAPACITY: usize = 5;

#[cfg(feature = "daemon")]
pub struct Daemon {
    io_front_end: io::SourceIO,
    clock_sync_algorithm: ClockSyncAlgorithm,
    receiver_stream: ReceiverStream,
    clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
    cancellation_token: CancellationToken,
    clock_state_handle: ClockStateHandle,
    dns_message_rx: mpsc::Receiver<DnsMessage>,
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
        let (dns_message_tx, dns_message_rx) = mpsc::channel(DAEMON_MESSAGE_CAPACITY);
        let mut io_front_end =
            io::SourceIO::construct(selected_clock.clone(), daemon_info, dns_message_tx);
        let clock_disruption_receiver = io_front_end.clock_disruption_receiver();

        let mut receiver_stream = ReceiverStream::default();
        let mut clock_sync_algorithm = ClockSyncAlgorithm::builder()
            .selected_clock(selected_clock.clone())
            .selector(Selector::new(MAX_DISPERSION_GROWTH))
            .build();

        let mut source_mutator = SourceMutator::new(
            &mut io_front_end,
            &mut clock_sync_algorithm,
            &mut receiver_stream,
        );

        // Platform auto-detection: determine which sources to install.
        let autodetect_result = match Autodetect::detect() {
            Ok(result) => result,
            Err(e) => {
                warn!(
                    ?e,
                    "Platform auto-detection failed; defaulting to non-Amazon."
                );
                Autodetect::Other
            }
        };

        // Install Amazon-conditional sources.
        source_mutator
            .init_from_autodetect_results(&autodetect_result, MAX_DISPERSION_GROWTH)
            .await;

        // Install universal NTP sources.
        for socket_addr in AWS_TEMP_PUBLIC_TIME_ADDRESSES {
            source_mutator.add_ntp_source(socket_addr, MAX_DISPERSION_GROWTH);
        }

        // Read VMClock state (may have been created by init_amazon_sources above).
        #[expect(clippy::redundant_closure_for_method_calls)]
        let disruption_marker = io_front_end
            .vmclock()
            .map(|vmclock| vmclock.last_disruption_marker())
            .unwrap_or_default();
        let clock_disruption_support_enabled = io_front_end.vmclock().is_some();

        // Initialize the Clock State component
        let (clock_state_tx, clock_state) = {
            let (tx, rx) = async_ring_buffer::create(1);
            let clock_state = ClockState::construct(
                rx,
                clock_disruption_receiver.clone(),
                clock_state_cancellation_token.clone(),
                disruption_marker,
                clock_disruption_support_enabled,
                ClockStatus::Unknown,
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
            dns_message_rx,
        }
    }

    /// Run the daemon.
    pub async fn run(mut self: Box<Self>) {
        self.clock_sync_algorithm.init_repro();
        // Start IO polling
        self.io_front_end.spawn_all();
        self.clock_state_handle.task_tracker.spawn({
            #[expect(
                clippy::missing_panics_doc,
                reason = "struct always initialized with `Some`"
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
                routable_event = self.receiver_stream.recv() => {
                    // TODO: This will return None if nothing is initialized. We need
                    // the handle this scenario
                    #[expect(clippy::missing_panics_doc)]
                    let routable_event = routable_event.unwrap();
                    self.handle_event(routable_event);
                }
                dns_message = self.dns_message_rx.recv() => {
                    #[expect(clippy::missing_panics_doc, reason = "tx held by SourceIO. Will not close")]
                    let dns_message = dns_message.unwrap();
                    self.handle_dns_message(dns_message);
                }
            }
        }
    }

    fn handle_event(&mut self, routable_event: RoutableEvent) {
        if let Some(source_params) = self.clock_sync_algorithm.feed(routable_event) {
            use crate::daemon::async_ring_buffer::SendError;

            match self.clock_state_handle.tx.send(source_params.clone()) {
                Ok(()) => (),
                Err(SendError::Disrupted(source_params)) => {
                    // don't handle_disruption. It will be handled on the next call of tokio::select
                    info!(
                        ?source_params,
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

    // Should only be called from the run fn
    fn handle_dns_message(&mut self, dns_message: DnsMessage) {
        let mut source_mutator = SourceMutator::new(
            &mut self.io_front_end,
            &mut self.clock_sync_algorithm,
            &mut self.receiver_stream,
        );
        match dns_message {
            DnsMessage::AddPoolAddr(msg) => {
                source_mutator.add_pool_source(msg.pool_domain, msg.addr, MAX_DISPERSION_GROWTH);
            }
            DnsMessage::RemovePoolAddr(msg) => {
                source_mutator.remove_pool_source(msg.pool_domain, msg.addr);
            }
        }
    }

    /// Handle a clock disruption event
    fn handle_disruption(&mut self) {
        // using destructure pattern ensure that all fields are handled
        let Self {
            io_front_end: _,
            clock_sync_algorithm,
            receiver_stream,
            clock_disruption_receiver,
            cancellation_token: _,
            clock_state_handle,
            dns_message_rx: _,
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
}

#[cfg(feature = "daemon")]
struct ClockStateHandle {
    clock_state: Option<ClockState>,
    tx: Sender<SyncParameters>,
    cancellation_token: CancellationToken,
    task_tracker: TaskTracker,
}
