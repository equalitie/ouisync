use std::time::Duration;

pub mod quic;
pub mod stun;
pub mod tcp;
pub mod udp;

#[cfg(not(feature = "simulation"))]
mod socket;

pub const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(10);
