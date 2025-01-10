use ouisync_lib::{self, LocalDiscovery, PeerPort};
use std::io;
use tokio::{net::UdpSocket, task};

#[tokio::main]
async fn main() -> io::Result<()> {
    env_logger::init();

    // Create a socket just to get a random and unique port, so that running two or more instances
    // of this utility don't clash.
    let sock = UdpSocket::bind("0.0.0.0:0").await?;
    let addr = sock.local_addr()?;

    let mut discovery = LocalDiscovery::new_mdns_direct(PeerPort::Quic(addr.port()));

    loop {
        let peer = discovery.recv().await;
        println!("Found peer {peer:?}");

        task::spawn(async move {
            peer.on_unseen().await;
            println!("Lost peer {peer:?}");
        });
    }
}
