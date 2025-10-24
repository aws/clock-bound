//! Ntp IO Source constants

use std::net::{Ipv4Addr, SocketAddrV4};
use tokio::time::Duration;

pub mod packet;
pub use packet::Packet;

pub const UNSPECIFIED_SOCKET_ADDRESS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
pub const LINK_LOCAL_ADDRESS: SocketAddrV4 =
    SocketAddrV4::new(Ipv4Addr::new(169, 254, 169, 123), 123);
pub const INTERVAL_DURATION: Duration = Duration::from_secs(1);
pub const LINK_LOCAL_TIMEOUT: Duration = Duration::from_millis(100);

pub const NTP_SERVER_INTERVAL_DURATION: Duration = Duration::from_secs(16);
pub const NTP_SERVER_TIMEOUT: Duration = Duration::from_millis(100);
