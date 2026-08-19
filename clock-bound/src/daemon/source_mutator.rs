//! One stop shop for mutating a source between the Clock Sync Algorithm and `SourceIO`
//!
//! Adding (and eventually removing) structs requires a bit of a song and dance between
//! [`ClockSyncAlgorithm`], [`SourceIO`], and [`ReceiverStream`]. This attempts to
//! codify this at runtime using references to these

use std::net::SocketAddr;

use crate::daemon::ClockSyncAlgorithm;
use crate::daemon::ReceiverStream;
use crate::daemon::async_ring_buffer;
use crate::daemon::autodetect::{self, Autodetect};
use crate::daemon::clock_sync_algorithm::source;
use crate::daemon::io::SourceIO;
use crate::daemon::time::tsc::Skew;
use crate::vmclock::shm::VMCLOCK_SHM_DEFAULT_PATH;
use tracing::info;

/// Mutate the sources in the app
///
/// For right now, all mutating actions need to happen before calling [`SourceIO::spawn_all`].
/// However, allowing this to work during runtime will be relatively easy in a future commit.
pub struct SourceMutator<'a> {
    io_front_end: &'a mut SourceIO,
    clock_sync_algorithm: &'a mut ClockSyncAlgorithm,
    receiver_stream: &'a mut ReceiverStream,
}

impl<'a> SourceMutator<'a> {
    pub fn new(
        io_front_end: &'a mut SourceIO,
        clock_sync_algorithm: &'a mut ClockSyncAlgorithm,
        receiver_stream: &'a mut ReceiverStream,
    ) -> Self {
        Self {
            io_front_end,
            clock_sync_algorithm,
            receiver_stream,
        }
    }

    /// Install sources based on platform autodetection results.
    ///
    /// On Amazon (Nitro or Xen), installs Amazon Time Sync, PHC, and VMClock (if non-metal).
    /// On other platforms, this is a no-op — only NTP sources are used, installed separately.
    pub async fn init_from_autodetect_results(
        &mut self,
        autodetect: &Autodetect,
        max_dispersion: Skew,
    ) {
        match autodetect {
            Autodetect::Amazon(amazon) => self.init_amazon_sources(amazon, max_dispersion).await,
            Autodetect::Other => {
                info!("Non-Amazon platform detected. Skipping Amazon-specific sources.");
            }
        }
    }

    /// Install all Amazon-specific clock sources: local Amazon Time Sync NTP, PHC, and VMClock.
    ///
    /// VMClock is only installed on non-metal instances
    async fn init_amazon_sources(&mut self, amazon: &autodetect::Amazon, max_dispersion: Skew) {
        self.set_amazon_time_sync(max_dispersion);
        self.set_phc(max_dispersion).await;

        // VMClock only touches the IO layer — it does not wire into the clock sync algorithm or
        // receiver stream. It provides clock disruption events, not time samples.
        if amazon.is_metal() {
            info!("EC2 metal instance detected. VMClock not enabled.");
        } else {
            // TODO: align create_vmclock error handling with PHC's graceful degradation (currently
            // panics if the shm file is missing on a confirmed non-metal EC2 instance).
            self.io_front_end
                .create_vmclock(VMCLOCK_SHM_DEFAULT_PATH)
                .await;
        }
    }

    /// Set the Amazon Time Sync source in the daemon
    pub fn set_amazon_time_sync(&mut self, max_dispersion: Skew) {
        self.clock_sync_algorithm
            .set_amazon_time_sync(source::AmazonTimeSync::new(max_dispersion));

        let (tx, rx) = async_ring_buffer::create(2);
        self.receiver_stream.set_amazon_time_sync(rx);
        self.io_front_end.create_amazon_time_sync(tx);
    }

    /// Set the Amazon PHC source in the daemon
    // TODO: make this function no longer async io moves to task runtime
    // TODO: This creates the io component first. This is not ideal, as it complicates
    // the logic of moving io initialization to the PHC IO task
    pub async fn set_phc(&mut self, max_dispersion_growth: Skew) {
        let (tx, rx) = async_ring_buffer::create(2);
        self.io_front_end.create_phc(tx).await;

        let Some(phc) = self.io_front_end.phc() else {
            tracing::info!(
                "PHC device was not initialized. Not installing into clock sync algorithm"
            );
            return;
        };
        let phc_path = source::DevicePath::from(phc.device_path());

        self.receiver_stream.set_phc(phc_path.clone(), rx);
        self.clock_sync_algorithm
            .set_phc(source::Phc::new(phc_path, max_dispersion_growth));
    }

    /// Add an NTP source in the daemon
    pub fn add_ntp_source(&mut self, socket_addr: SocketAddr, max_dispersion: Skew) {
        self.clock_sync_algorithm
            .add_ntp_source(source::NtpSource::new_ip_addr_source(
                socket_addr,
                max_dispersion,
            ));

        let (tx, rx) = async_ring_buffer::create(2);
        self.receiver_stream.add_ntp_source((socket_addr, rx));
        self.io_front_end.create_ip_addr_source((socket_addr, tx));
    }

    /// Add a multi-source NTP pool to the daemon.
    ///
    /// The pool resolves `pool_domain` and maintains up to 5 healthy NTP sources
    /// concurrently, quarantining and rotating individual addresses as they fail.
    /// Registers the pool in IO so that resolved sources can later be added
    /// via [`add_pool_source`](Self::add_pool_source).
    ///
    /// For a domain that should back only a single NTP source, use
    /// [`add_domain_host`](Self::add_domain_host).
    pub fn add_pool(&mut self, pool_domain: String) {
        /// Maximum healthy sources maintained for a multi-source pool.
        const MAX_SOURCES: usize = 5;
        self.io_front_end.create_pool(pool_domain, MAX_SOURCES);
    }

    /// Add a single-host NTP source addressed by domain name.
    ///
    /// This is a DNS-backed pool capped at a single healthy source: it resolves
    /// `pool_domain` and maintains one live NTP source at a time, inheriting the
    /// pool's quarantine-and-rotate behavior so a failing address is replaced by
    /// another resolved address. It differs from [`add_ntp_source`](Self::add_ntp_source),
    /// which pins to a fixed IP, and from [`add_pool`](Self::add_pool), which
    /// maintains multiple concurrent sources.
    pub fn add_domain_host(&mut self, pool_domain: String) {
        /// Maximum healthy sources maintained for a single-host domain.
        const MAX_SOURCES: usize = 1;
        self.io_front_end.create_pool(pool_domain, MAX_SOURCES);
    }

    /// Add a source to a pool
    pub fn add_pool_source(
        &mut self,
        pool_domain: &str,
        socket_addr: SocketAddr,
        max_dispersion: Skew,
    ) {
        let source =
            source::NtpSource::new_with_pool(socket_addr, pool_domain.to_owned(), max_dispersion);
        self.clock_sync_algorithm.add_ntp_source(source);

        let (tx, rx) = async_ring_buffer::create(2);
        self.receiver_stream.add_ntp_source((socket_addr, rx));
        self.io_front_end
            .add_pool_source(pool_domain, socket_addr, tx);
    }

    /// Remove a source from a pool
    pub async fn remove_pool_source(&mut self, pool_domain: &str, socket_addr: SocketAddr) {
        self.clock_sync_algorithm.remove_ntp_source(&socket_addr);
        self.receiver_stream.remove_ntp_source(&socket_addr);
        self.io_front_end
            .remove_pool_source(pool_domain, &socket_addr)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::str::FromStr;
    use std::sync::Arc;

    use tokio::sync::mpsc;

    use crate::daemon::ClockSyncAlgorithm;
    use crate::daemon::clock_sync_algorithm::Selector;
    use crate::daemon::io::SourceIO;
    use crate::daemon::io::ntp::DaemonInfo;
    use crate::daemon::message::Dns as DnsMessage;
    use crate::daemon::receiver_stream::ReceiverStream;
    use crate::daemon::selected_clock::SelectedClockSource;
    use crate::daemon::time::tsc::Skew;

    use super::SourceMutator;

    /// Helper to construct the trio of objects needed by `SourceMutator`.
    fn setup() -> (SourceIO, ClockSyncAlgorithm, ReceiverStream) {
        let selected_clock = Arc::new(SelectedClockSource::default());
        let daemon_info = DaemonInfo {
            major_version: 2,
            minor_version: 100,
            startup_id: 0xDEAD_BEEF_CAFE_BABE,
        };

        let (dns_message_tx, _dns_message_rx) = mpsc::channel::<DnsMessage>(1);
        let io = SourceIO::construct(selected_clock.clone(), daemon_info, dns_message_tx);
        let csa = ClockSyncAlgorithm::builder()
            .selected_clock(selected_clock)
            .selector(Selector::new(Skew::from_ppm(15.0)))
            .build();
        let receiver_stream = ReceiverStream::default();

        (io, csa, receiver_stream)
    }

    #[tokio::test]
    async fn set_amazon_time_sync_populates_all_components() {
        let (mut io, mut csa, mut rs) = setup();

        assert!(csa.amazon_time_sync().is_none());
        assert!(rs.amazon_time_sync().is_none());

        let mut mutator = SourceMutator::new(&mut io, &mut csa, &mut rs);
        mutator.set_amazon_time_sync(Skew::from_ppm(15.0));

        assert!(csa.amazon_time_sync().is_some());
        assert!(rs.amazon_time_sync().is_some());
    }

    #[tokio::test]
    async fn add_ntp_source_populates_all_components() {
        let (mut io, mut csa, mut rs) = setup();

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 123);

        assert!(csa.ntp_sources().is_empty());

        let mut mutator = SourceMutator::new(&mut io, &mut csa, &mut rs);
        mutator.add_ntp_source(addr, Skew::from_ppm(15.0));

        assert_eq!(csa.ntp_sources().len(), 1);
        assert_eq!(csa.ntp_sources()[0].socket_address(), addr);
    }

    #[tokio::test]
    async fn add_multiple_ntp_sources() {
        let (mut io, mut csa, mut rs) = setup();

        let addr1 = SocketAddr::from_str("192.0.1.1:123").unwrap();
        let addr2 = SocketAddr::from_str("192.0.2.2:123").unwrap();

        {
            let mut mutator = SourceMutator::new(&mut io, &mut csa, &mut rs);
            mutator.add_ntp_source(addr1, Skew::from_ppm(15.0));
            mutator.add_ntp_source(addr2, Skew::from_ppm(10.0));
        }

        assert_eq!(csa.ntp_sources().len(), 2);
        assert_eq!(csa.ntp_sources()[0].socket_address(), addr1);
        assert_eq!(csa.ntp_sources()[1].socket_address(), addr2);
    }

    #[test]
    fn add_pool_populates_io() {
        let (mut io, mut csa, mut rs) = setup();
        let pool_domain = "pool.ntp.org".to_string();

        let mut mutator = SourceMutator::new(&mut io, &mut csa, &mut rs);
        mutator.add_pool(pool_domain.clone());

        // Verify pool exists in IO
        assert!(io.pools().contains_key(&pool_domain));
    }
}
