//! One stop shop for mutating a source between the Clock Sync Algorithm and `SourceIO`
//!
//! Adding (and eventually removing) structs requires a bit of a song and dance between
//! [`ClockSyncAlgorithm`], [`SourceIO`], and [`ReceiverStream`]. This attempts to
//! codify this at runtime using references to these

use std::net::SocketAddr;

use crate::daemon::ClockSyncAlgorithm;
use crate::daemon::ReceiverStream;
use crate::daemon::async_ring_buffer;
use crate::daemon::clock_sync_algorithm::source;
use crate::daemon::io::SourceIO;
use crate::daemon::time::tsc::Skew;

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

    /// Set the Amazon Time Sync source in the daemon
    // TODO: this function is async due to `io_front_end.create_amazon_time_sync` binds a socket. That should not happen here
    pub async fn set_amazon_time_sync(&mut self, max_dispersion: Skew) {
        self.clock_sync_algorithm
            .set_amazon_time_sync(source::AmazonTimeSync::new(max_dispersion));

        let (tx, rx) = async_ring_buffer::create(2);
        self.receiver_stream.set_amazon_time_sync(rx);
        self.io_front_end.create_amazon_time_sync(tx).await;
    }

    /// Set the Amazon PHC source in the daemon
    // TODO: make this function no longer async io moves to task runtime
    // TODO: This creates the io component first. This is not ideal, as it complicates
    // the logic of moving io initialization to the PHC IO task
    pub async fn set_phc(&mut self, max_dispersion_growth: Skew) {
        let (tx, rx) = async_ring_buffer::create(2);
        self.receiver_stream.set_phc(rx);
        self.io_front_end.create_phc(tx).await;

        #[expect(clippy::missing_panics_doc, reason = "built in line above")]
        let phc = self.io_front_end.phc().unwrap();
        let phc_path = phc.device_path().to_owned();

        self.clock_sync_algorithm
            .set_phc(source::Phc::new(phc_path, max_dispersion_growth));
    }

    /// Add an NTP source in the daemon
    ///
    /// TODO: Make this function non-async by moving socket binding to spawned task
    pub async fn add_ntp_source(&mut self, socket_addr: SocketAddr, max_dispersion: Skew) {
        self.clock_sync_algorithm
            .add_ntp_source(source::NtpSource::new(socket_addr, max_dispersion));

        let (tx, rx) = async_ring_buffer::create(2);
        self.receiver_stream.add_ntp_source((socket_addr, rx));
        self.io_front_end.create_ntp_source((socket_addr, tx)).await;
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;

    use crate::daemon::ClockSyncAlgorithm;
    use crate::daemon::clock_sync_algorithm::Selector;
    use crate::daemon::io::SourceIO;
    use crate::daemon::io::ntp::DaemonInfo;
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

        let io = SourceIO::construct(selected_clock.clone(), daemon_info);
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
        mutator.set_amazon_time_sync(Skew::from_ppm(15.0)).await;

        assert!(csa.amazon_time_sync().is_some());
        assert!(rs.amazon_time_sync().is_some());
    }

    #[tokio::test]
    async fn add_ntp_source_populates_all_components() {
        let (mut io, mut csa, mut rs) = setup();

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 123);

        assert!(csa.ntp_sources.is_empty());

        let mut mutator = SourceMutator::new(&mut io, &mut csa, &mut rs);
        mutator.add_ntp_source(addr, Skew::from_ppm(15.0)).await;

        assert_eq!(csa.ntp_sources.len(), 1);
        assert_eq!(csa.ntp_sources[0].socket_address(), addr);
    }

    #[tokio::test]
    async fn add_multiple_ntp_sources() {
        let (mut io, mut csa, mut rs) = setup();

        let addr1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 123);
        let addr2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2)), 123);

        {
            let mut mutator = SourceMutator::new(&mut io, &mut csa, &mut rs);
            mutator.add_ntp_source(addr1, Skew::from_ppm(15.0)).await;
            mutator.add_ntp_source(addr2, Skew::from_ppm(10.0)).await;
        }

        assert_eq!(csa.ntp_sources.len(), 2);
        assert_eq!(csa.ntp_sources[0].socket_address(), addr1);
        assert_eq!(csa.ntp_sources[1].socket_address(), addr2);
    }
}
