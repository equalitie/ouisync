use btdht::MainlineDht;
use futures_util::StreamExt;
use ouisync_lib::{
    network::{self, dht_discovery},
    ShareToken,
};
use std::{collections::HashSet, io, net::{Ipv4Addr, Ipv6Addr}};
use structopt::StructOpt;
use tokio::{net::UdpSocket, task};

/// Command line options.
#[derive(StructOpt, Debug)]
struct Options {
    /// Accept a share token.
    #[structopt(long, value_name = "TOKEN")]
    pub token: ShareToken,
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let options = Options::from_args();

    env_logger::init();

    let socket_v4 = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await.ok();
    let socket_v6 = UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).await.ok();

    println!("Resolving DHT routers...");
    let (ipv4_routers, ipv6_routers) = dht_discovery::dht_router_addresses().await;

    for router in ipv4_routers.iter().chain(ipv6_routers.iter()) {
        println!("  {:?}", router);
    }

    let dht_v4 = socket_v4.map(|socket|
        MainlineDht::builder()
            .add_routers(ipv4_routers)
            .set_read_only(true)
            .start(socket)
    );

    let dht_v6 = socket_v6.map(|socket|
        MainlineDht::builder()
            .add_routers(ipv6_routers)
            .set_read_only(true)
            .start(socket)
    );

    let task = task::spawn(async move {
        if let Some(dht) = dht_v4 {
            println!("");
            lookup("IPv4", &dht, &options.token).await;
        }

        if let Some(dht) = dht_v6 {
            println!("");
            lookup("IPv6", &dht, &options.token).await;
        }
    });

    task.await?;

    Ok(())
}

async fn lookup(prefix: &str, dht: &MainlineDht, token: &ShareToken) {
    println!("{} Bootstrapping...", prefix);
    
    if dht.bootstrapped().await {
        let mut seen_peers = HashSet::new();
        let info_hash = network::repository_info_hash(token.id());
    
        println!("{} Searching for peers...", prefix);
        let mut peers = dht.search(info_hash, false);
    
        while let Some(peer) = peers.next().await {
            if seen_peers.insert(peer) {
                println!("  {:?}", peer);
            }
        }
    } else {
        println!("{} Bootstrap failed", prefix)
    }
}
