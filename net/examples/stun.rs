use clap::Parser;
use ouisync_net::{stun::StunClient, udp::UdpSocket};
use std::{
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
};
use tokio::net;

#[tokio::main]
async fn main() -> io::Result<()> {
    let options = Options::parse();

    let client_v4 = StunClient::new(UdpSocket::bind(
        (Ipv4Addr::UNSPECIFIED, options.port).into(),
    )?);
    let client_v6 = StunClient::new(UdpSocket::bind(
        (Ipv6Addr::UNSPECIFIED, options.port).into(),
    )?);

    for server_name in options.servers {
        for server_addr in net::lookup_host(server_name.clone()).await? {
            println!("STUN server {server_name} ({server_addr}):");

            let client = match server_addr {
                SocketAddr::V4(_) => &client_v4,
                SocketAddr::V6(_) => &client_v6,
            };

            let result = client.external_addr(server_addr).await;
            println!("  external address:       {}", format_display(result));

            let result = client.nat_filtering(server_addr).await;
            println!("  NAT filtering behavior: {}", format_debug(result));

            let result = client.nat_mapping(server_addr).await;
            println!("  NAT mapping behavior:   {}", format_debug(result));

            println!();
        }
    }

    Ok(())
}

#[derive(Parser, Debug)]
struct Options {
    #[arg(short, long, default_value_t = 0)]
    port: u16,

    #[arg(short, long, default_value = "stun1.l.google.com:19305")]
    servers: Vec<String>,
}

fn format_debug<T: std::fmt::Debug>(result: io::Result<T>) -> String {
    match result {
        Ok(value) => format!("{value:?}"),
        Err(err) => format!("Error: {err}"),
    }
}

fn format_display<T: std::fmt::Display>(result: io::Result<T>) -> String {
    match result {
        Ok(value) => format!("{value}"),
        Err(err) => format!("Error: {err}"),
    }
}
