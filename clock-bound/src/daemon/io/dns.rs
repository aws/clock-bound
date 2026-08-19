//! DNS IO specific code and implementations

// IO task which holds the state of the DNS pool and performs DNS resolve actions
pub mod resolver;

// IO task which performs NTP requests to a DNS-resolved address
pub mod ntp_source;

use std::collections::HashMap;
use std::net::SocketAddr;

use tokio::sync::mpsc;
use tokio_util::task::TaskTracker;
use tracing::{debug, error, info, warn};

use super::{ControlRequest, Source, SourceState};
use crate::daemon::message::Dns as DnsMessage;
use ntp_source::NtpSource;
use resolver::{Message as ResolverMessage, Resolver};

/// Struct that lives in [`SourceIO`](super::SourceIO)
/// to act as the interface to a DNS pool
#[derive(Debug)]
pub struct Pool {
    domain: String,
    /// Live NTP source tasks for this pool, keyed by socket address
    ntp_sources: HashMap<SocketAddr, Source<NtpSource>>,
    /// The resolver task for this pool
    resolver: Source<Resolver>,
    /// Sender half of the resolver message channel — cloned into each `NtpSource`
    resolver_message_tx: mpsc::Sender<ResolverMessage>,
}

impl Pool {
    /// Constructs a new `Pool` with all internal channels and a ready-to-spawn Resolver.
    ///
    /// `max_sources` is the maximum number of healthy NTP tasks the pool's
    /// resolver maintains.
    pub fn construct(
        pool_domain: String,
        daemon_tx: mpsc::Sender<DnsMessage>,
        max_sources: usize,
    ) -> Self {
        // Channel for NTP sources to communicate with the Resolver
        let (resolver_message_tx, ntp_receiver) = mpsc::channel::<ResolverMessage>(16);

        // Control channel for the Resolver
        let (ctrl_sender, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);

        let resolver = Resolver::construct(
            pool_domain.clone(),
            daemon_tx,
            ctrl_receiver,
            ntp_receiver,
            max_sources,
        );

        Self {
            domain: pool_domain,
            ntp_sources: HashMap::new(),
            resolver: Source {
                state: SourceState::Initialized(resolver),
                ctrl_sender,
            },
            resolver_message_tx,
        }
    }

    /// Returns a clone of the resolver message sender for use by NTP sources.
    pub fn resolver_message_sender(&self) -> mpsc::Sender<ResolverMessage> {
        self.resolver_message_tx.clone()
    }

    /// Inserts a running NTP source into the pool.
    ///
    /// # Panics
    /// Panics if the address is already in the pool
    pub(super) fn add_source(&mut self, addr: SocketAddr, source: Source<NtpSource>) {
        let inserted = self.ntp_sources.insert(addr, source);
        if inserted.is_some() {
            error!(pool_domain = self.domain, %addr, "Attempted to add duplicate addr to pool");
            panic!("Attempted to add an already-existing pool source");
        }
    }

    /// Removes an NTP source from the pool, returning it.
    ///
    /// # Panics
    /// Panics if the address is not in the pool
    pub(super) fn remove_source(&mut self, addr: &SocketAddr) -> Source<NtpSource> {
        let removed = self.ntp_sources.remove(addr);

        if let Some(removed) = removed {
            removed
        } else {
            error!(pool_domain = self.domain, %addr, "Attempted to remove non-existent addr from pool");
            panic!("Attempted to remove a pool source that does not exist");
        }
    }

    /// Spawns the Resolver task on the given tracker.
    ///
    /// # Panics
    /// Panics if the NTP source `HashMap` is not empty (invariant: no sources exist before
    /// the Resolver has started), or if the Resolver has already been spawned.
    #[tracing::instrument(level = "info", skip_all, fields(pool = %self.domain))]
    pub fn spawn_resolver(&mut self, tracker: &TaskTracker) {
        assert!(
            self.ntp_sources.is_empty(),
            "NTP source HashMap must be empty when spawning the Resolver"
        );

        debug!("Attempting to spawn Resolver.");
        if let SourceState::Initialized(mut resolver) = self.resolver.state.transition_to_running()
        {
            tracker.spawn(async move { resolver.run().await });
            info!("Successfully spawned Resolver.");
        } else {
            warn!("Attempted to spawn Resolver when one is already running.");
        }
    }

    /// Returns the number of NTP sources in this pool.
    pub fn ntp_source_count(&self) -> usize {
        self.ntp_sources.len()
    }

    /// Gracefully shuts down the pool: NTP sources first, then the Resolver.
    ///
    /// NTP sources are shut down before the Resolver to ensure they stop sending
    /// messages on `resolver_tx` before the Resolver exits and drops its receiver.
    pub async fn shutdown(&mut self) {
        // Shutdown all NTP sources first
        // take to iterate while draining hashmap
        let ntp_sources = std::mem::take(&mut self.ntp_sources);
        for (address, source) in ntp_sources {
            #[expect(
                clippy::missing_panics_doc,
                reason = "NTP task can only be shutdown from here. No race possible"
            )]
            source
                .ctrl_sender
                .send(ControlRequest::Shutdown)
                .await
                .unwrap();
            info!(%address, pool = %self.domain, "Sent shutdown to DNS pool NTP source");
        }

        // Shutdown the Resolver
        #[expect(
            clippy::missing_panics_doc,
            reason = "resolver lives for entire app lifetime"
        )]
        self.resolver
            .ctrl_sender
            .send(ControlRequest::Shutdown)
            .await
            .unwrap();
    }
}
