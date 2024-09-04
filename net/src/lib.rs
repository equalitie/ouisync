use std::time::Duration;

pub mod connection;
pub mod quic;
pub mod stun;
pub mod tcp;
pub mod udp;

#[cfg(not(feature = "simulation"))]
mod socket;
mod sync;

pub const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(10);
