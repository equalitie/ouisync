// TODO: remove this when we upgrade clap to v4.0
#![allow(deprecated)]

use super::peer_addr::PeerAddr;
use clap::Parser;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

#[derive(Parser, Debug)]
pub struct NetworkOptions {
    /// Addresses to bind to. The expected format is {tcp,quic}/IP:PORT. Note that there may be at
    /// most one of each (protoco x IP-version) combinations. If more are specified, only the first
    /// one is used.
    #[clap(long, default_values = &["quic/0.0.0.0:0", "quic/[::]:0"], value_name = "proto/ip:port")]
    pub bind: Vec<PeerAddr>,

    /// Disable local discovery
    #[clap(short, long)]
    pub disable_local_discovery: bool,

    /// Disable UPnP
    #[clap(long)]
    pub disable_upnp: bool,

    /// Disable DHT
    #[clap(long)]
    pub disable_dht: bool,

    /// Explicit list of {tcp,quic}/IP:PORT addresses of peers to connect to
    #[clap(long)]
    pub peers: Vec<PeerAddr>,
}

impl NetworkOptions {
    pub fn listen_tcp_addr_v4(&self) -> Option<SocketAddr> {
        self.bind.iter().find_map(|addr| match addr {
            PeerAddr::Tcp(addr @ SocketAddr::V4(_)) => Some(*addr),
            _ => None,
        })
    }

    pub fn listen_tcp_addr_v6(&self) -> Option<SocketAddr> {
        self.bind.iter().find_map(|addr| match addr {
            PeerAddr::Tcp(addr @ SocketAddr::V6(_)) => Some(*addr),
            _ => None,
        })
    }

    pub fn listen_quic_addr_v4(&self) -> Option<SocketAddr> {
        self.bind.iter().find_map(|addr| match addr {
            PeerAddr::Quic(addr @ SocketAddr::V4(_)) => Some(*addr),
            _ => None,
        })
    }

    pub fn listen_quic_addr_v6(&self) -> Option<SocketAddr> {
        self.bind.iter().find_map(|addr| match addr {
            PeerAddr::Quic(addr @ SocketAddr::V6(_)) => Some(*addr),
            _ => None,
        })
    }
}

impl Default for NetworkOptions {
    fn default() -> Self {
        Self {
            bind: vec![
                PeerAddr::Quic((Ipv4Addr::UNSPECIFIED, 0).into()),
                PeerAddr::Quic((Ipv6Addr::UNSPECIFIED, 0).into()),
            ],
            disable_local_discovery: false,
            disable_upnp: false,
            disable_dht: false,
            peers: Vec::new(),
        }
    }
}
