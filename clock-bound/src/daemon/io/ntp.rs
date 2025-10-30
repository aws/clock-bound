//! Ntp IO Source constants

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use tokio::time::Duration;

pub mod packet;
pub use packet::Packet;

pub const UNSPECIFIED_SOCKET_ADDRESS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
pub const LINK_LOCAL_ADDRESS: SocketAddrV4 =
    SocketAddrV4::new(Ipv4Addr::new(169, 254, 169, 123), 123);
pub const LINK_LOCAL_INTERVAL_DURATION: Duration = Duration::from_secs(1);
pub const LINK_LOCAL_TIMEOUT: Duration = Duration::from_millis(100);

pub const FIRST_PUBLIC_TIME_ADDRESS: std::net::SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(166, 117, 111, 42)), 123);
pub const SECOND_PUBLIC_TIME_ADDRESS: std::net::SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(3, 33, 186, 244)), 123);
pub const NTP_SOURCE_INTERVAL_DURATION: Duration = Duration::from_secs(16);
pub const NTP_SOURCE_TIMEOUT: Duration = Duration::from_millis(100);
