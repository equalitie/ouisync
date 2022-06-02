use btdht::MainlineDht;
use futures_util::StreamExt;
use ouisync_lib::{
    network::{self, dht_discovery::DHT_ROUTERS},
    ShareToken,
};
use std::{
    collections::HashSet,
    io,
    net::{Ipv4Addr, Ipv6Addr, IpAddr},
};
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

    let socket_v4 = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .ok()
        .and_then(|s| btdht::Socket::new(s).ok());

    // Note: [BEP-32](https://www.bittorrent.org/beps/bep_0032.html) says we should bind the ipv6
    // socket to a concrete unicast address, not to an unspecified one.
    let socket_v6 = match network::dht_discovery::local_ipv6_address().await {
        Some(ipv6_addr) => {
            UdpSocket::bind((ipv6_addr, 0))
                .await
                .ok()
                .and_then(|socket| btdht::Socket::new(socket).ok())
        },
        None => None,
    };

    let dht_v4 = socket_v4.map(|socket| {
        MainlineDht::builder()
            .add_routers(DHT_ROUTERS.iter().copied())
            .set_read_only(true)
            .start(socket)
    });

    let dht_v6 = socket_v6.map(|socket| {
        MainlineDht::builder()
            .add_routers(DHT_ROUTERS.iter().copied())
            .set_read_only(true)
            .start(socket)
    });

    let task = task::spawn(async move {
        if let Some(dht) = dht_v4 {
            println!();
            lookup("IPv4", &dht, &options.token).await;
        }

        if let Some(dht) = dht_v6 {
            println!();
            lookup("IPv6", &dht, &options.token).await;
        }
    });

    task.await?;

    // Uncomment to keep observing the DHT (for when debugging).
    //std::future::pending::<()>().await;

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
