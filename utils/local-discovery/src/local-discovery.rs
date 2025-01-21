use ouisync_lib::{self, LocalDiscovery, PeerPort};
use std::io;
use tokio::{net::UdpSocket, task};
use clap::{Parser, Subcommand};

#[derive(Subcommand, Debug)]
enum DiscoveryType {
    PoorMan,
    DirectMDNS,
    ZeroconfMDNS,
}

/// Command line options.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Type of discovery to use.
    #[command(subcommand)]
    pub discovery_type: DiscoveryType,
}

#[tokio::main]
async fn main() -> io::Result<()> {
    env_logger::init();

    let options = Args::parse();

    // Create a socket just to get a random and unique port, so that running two or more instances
    // of this utility don't clash.
    let sock = UdpSocket::bind("0.0.0.0:0").await?;
    let addr = sock.local_addr()?;

    println!("Our port is {:?}", addr.port());

    let port = PeerPort::Quic(addr.port());

    let mut discovery = match options.discovery_type {
        DiscoveryType::PoorMan => LocalDiscovery::new(port, None),
        DiscoveryType::DirectMDNS => LocalDiscovery::new_mdns_direct(port).unwrap(),
        DiscoveryType::ZeroconfMDNS => LocalDiscovery::new_mdns_zeroconf(port),
    };

    loop {
        let peer = discovery.recv().await;
        println!("{}: Found peer {:?}", time_str(), peer.initial_addr());

        task::spawn(async move {
            peer.on_unseen().await;
            println!("{}: Lost peer {:?}", time_str(), peer.initial_addr());
        });
    }
}

fn time_str() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}
