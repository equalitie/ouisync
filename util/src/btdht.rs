use btdht::MainlineDht;
use futures_util::StreamExt;
use ouisync_lib::{
    network::{self, dht_discovery},
    ShareToken,
};
use std::{collections::HashSet, io, net::Ipv4Addr};
use structopt::StructOpt;
use tokio::{net::UdpSocket, task};

/// Command line options.
#[derive(StructOpt, Debug)]
pub(crate) struct Options {
    /// Accept a share token.
    #[structopt(long, value_name = "TOKEN")]
    pub token: ShareToken,
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let options = Options::from_args();

    env_logger::init();

    // TODO: IPv6

    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;

    println!("Resolving DHT routers...");
    let (ipv4_routers, _ipv6_routers) = dht_discovery::dht_router_addresses().await;

    let dht = MainlineDht::builder()
        .add_routers(ipv4_routers)
        .set_read_only(true)
        .start(socket);

    let task = task::spawn( async move {
        println!("Bootstrapping...");

        if dht.bootstrapped().await {
            println!("Bootstrap complete");
            let info_hash = network::repository_info_hash(options.token.id());

            println!("Searching for peers...");
            let mut seen_peers = HashSet::new();
            let mut peers = dht.search(info_hash, false);

            while let Some(peer) = peers.next().await {
                if seen_peers.insert(peer) {
                    println!("    {:?}", peer);
                }
            }

            println!("Done");
        } else {
            println!("Bootstrap failed")
        }
    });

    task.await?;

    Ok(())
}
