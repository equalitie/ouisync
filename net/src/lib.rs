use std::time::Duration;

pub mod quic;
pub mod stun;
pub mod tcp;
pub mod udp;
pub mod unified;

#[cfg(not(feature = "simulation"))]
mod socket;
mod sync;

pub use socket::SocketOptions;

pub const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(10);
