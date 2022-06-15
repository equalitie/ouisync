// TODO: remove this when we upgrade clap to v4.0
#![allow(deprecated)]

use clap::Parser;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

#[derive(Parser, Debug)]
pub struct NetworkOptions {
    /// Port to listen on (0 for random)
    #[clap(short, long, default_value = "0")]
    pub port: u16,

    /// IPv4 address to bind to
    #[clap(long, default_value = "0.0.0.0", value_name = "ip")]
    pub bind_v4: Ipv4Addr,

    /// IPv6 address to bind to
    #[clap(long, default_value = "::", value_name = "ip")]
    pub bind_v6: Ipv6Addr,

    /// Disable local discovery
    #[clap(short, long)]
    pub disable_local_discovery: bool,

    /// Disable UPnP
    #[clap(long)]
    pub disable_upnp: bool,

    /// Disable DHT
    #[clap(long)]
    pub disable_dht: bool,

    /// Explicit list of IP:PORT pairs of peers to connect to
    #[clap(long)]
    pub peers: Vec<SocketAddr>,
}

impl NetworkOptions {
    pub fn listen_addr_v4(&self) -> SocketAddr {
        SocketAddr::new(self.bind_v4.into(), self.port)
    }

    pub fn listen_addr_v6(&self) -> SocketAddr {
        SocketAddr::new(self.bind_v6.into(), self.port)
    }
}

impl Default for NetworkOptions {
    fn default() -> Self {
        Self {
            port: 0,
            bind_v4: Ipv4Addr::UNSPECIFIED,
            bind_v6: Ipv6Addr::UNSPECIFIED,
            disable_local_discovery: false,
            disable_upnp: false,
            disable_dht: false,
            peers: Vec::new(),
        }
    }
}
