use clap::{Parser, Subcommand};
use ouisync_lib::{self, LocalDiscovery, PeerPort};
use std::io;
use tokio::{net::UdpSocket, task};

#[derive(Subcommand, Debug)]
enum DiscoveryType {
    /// Legacy local discovery implementation (custom protocol).
    PoorMan,
    /// mDNS implementation where this app does the multicasting.
    DirectMDNS,
    /// mDNS implementation where we connect to a Zeroconf (Avahi or Bonjour) daemon, which does
    /// the multicasting for us.
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

    println!("This app's port is UDP/{:?}", addr.port());
    println!("Using {:?} discovery type", options.discovery_type);

    let port = PeerPort::Quic(addr.port());

    let mut discovery = match options.discovery_type {
        DiscoveryType::PoorMan => LocalDiscovery::new_poor_man(port),
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

// Useful for debugging how long it takes for a service to disappear.
fn time_str() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}
