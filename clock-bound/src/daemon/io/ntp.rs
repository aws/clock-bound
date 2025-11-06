//! Ntp IO Source constants

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use tokio::time::Duration;

pub mod packet;
pub use packet::{Fec2V1Value as DaemonInfo, Packet};

use crate::daemon::{async_ring_buffer, event};

pub const LINK_LOCAL_BURST_DURATION: Duration = Duration::from_secs(1);

pub const UNSPECIFIED_SOCKET_ADDRESS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
pub const LINK_LOCAL_ADDRESS: SocketAddrV4 =
    SocketAddrV4::new(Ipv4Addr::new(169, 254, 169, 123), 123);
pub const LINK_LOCAL_INTERVAL_DURATION: Duration = Duration::from_secs(2);
pub const LINK_LOCAL_TIMEOUT: Duration = Duration::from_millis(100);
pub const AWS_TEMP_PUBLIC_TIME_ADDRESSES: [SocketAddr; 2] = [
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(166, 117, 111, 42)), 123),
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(3, 33, 186, 244)), 123),
];
pub const NTP_SOURCE_INTERVAL_DURATION: Duration = Duration::from_secs(16);
pub const NTP_SOURCE_TIMEOUT: Duration = Duration::from_millis(100);

/// Tuple to hold both the `SocketAddr` and ring buffer `Sender` for an IO `NTPSource`
pub type NTPSourceSender = (SocketAddr, async_ring_buffer::Sender<event::Ntp>);
/// Tuple to hold both the `SocketAddr` and ring buffer `Receiver` for an IO `NTPSource`
pub type NTPSourceReceiver = (SocketAddr, async_ring_buffer::Receiver<event::Ntp>);
