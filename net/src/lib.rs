pub mod quic;
pub mod tcp;
pub mod udp;

#[cfg(not(feature = "simulation"))]
mod socket;
