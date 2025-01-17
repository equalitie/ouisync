use ouisync_lib::{self, LocalDiscovery, PeerPort};
use std::io;
use tokio::{net::UdpSocket, task};

enum DiscoveryType {
    PoorMan,
    DirectMDNS,
    ZeroconfMDNS,
}

#[tokio::main]
async fn main() -> io::Result<()> {
    env_logger::init();

    let args: Vec<String> = std::env::args().collect();

    let discovery_type = if args.len() == 2 {
        match args[1].as_str() {
            "poor_man" => DiscoveryType::PoorMan,
            "direct_mdns" => DiscoveryType::DirectMDNS,
            "zeroconf_mdns" => DiscoveryType::ZeroconfMDNS,
            _ => {
                println!("Wrong args");
                return Ok(());
            }
        }
    } else {
        println!("Wrong args");
        return Ok(());
    };

    println!("ARGS: {args:?}");
    // Create a socket just to get a random and unique port, so that running two or more instances
    // of this utility don't clash.
    let sock = UdpSocket::bind("0.0.0.0:0").await?;
    let addr = sock.local_addr()?;

    let port = PeerPort::Quic(addr.port());

    let mut discovery = match discovery_type {
        DiscoveryType::PoorMan => LocalDiscovery::new(port, None),
        DiscoveryType::DirectMDNS => LocalDiscovery::new_mdns_direct(port),
        DiscoveryType::ZeroconfMDNS => LocalDiscovery::new_mdns_zeroconf(port),
    };

    loop {
        let peer = discovery.recv().await;
        println!("Found peer {:?}", peer.initial_addr());

        task::spawn(async move {
            peer.on_unseen().await;
            println!("Lost peer {:?}", peer.initial_addr());
        });
    }
}
