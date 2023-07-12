use btdht::{InfoHash, MainlineDht};
use futures_util::StreamExt;
use ouisync_lib::{
    network::{self, dht_discovery::DHT_ROUTERS},
    ShareToken,
};
use std::{
    collections::HashSet,
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
};
use structopt::StructOpt;
use tokio::{net::UdpSocket, task};

#[derive(StructOpt, Debug)]
enum Single {
    Lookup { peer: SocketAddr },
    Announce { peer: SocketAddr },
}

#[derive(StructOpt, Debug)]
enum Action {
    Lookup,
    Announce,
    // Just send a request to a single node, no need to bootstrap
    Single(Single),
}

/// Command line options.
#[derive(StructOpt, Debug)]
struct Options {
    /// Accept a share token.
    #[structopt(long, value_name = "TOKEN")]
    pub token: Option<ShareToken>,
    /// Unhashed swarm name, we hash it to get the info-hash.
    #[structopt(long)]
    pub swarm_name: Option<String>,
    /// Action to perform.
    #[structopt(subcommand)]
    pub action: Action,
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let options = Options::from_args();

    env_logger::init();

    let info_hash = if let Some(token) = &options.token {
        Some(network::repository_info_hash(token.id()))
    } else if let Some(swarm_name) = &options.swarm_name {
        Some(InfoHash::sha1(swarm_name.as_bytes()))
    } else {
        None
    };

    if let Action::Single(single_action) = &options.action {
        run_single_action(info_hash.unwrap(), single_action).await;
        return Ok(());
    }

    const WITH_IPV4: bool = true;
    const WITH_IPV6: bool = true;

    let socket_v4 = if WITH_IPV4 {
        UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await.ok()
    } else {
        None
    };

    // Note: [BEP-32](https://www.bittorrent.org/beps/bep_0032.html) says we should bind the ipv6
    // socket to a concrete unicast address, not to an unspecified one. Not sure it's worth it
    // though as (1) I'm not sure how multi-homing causes problems and (2) devices often change IP
    // addresses (switch to a different wifi, or cellular,...) so we would need a mechanism to
    // restart the DHT with a different socket if that happens.
    let socket_v6 = if WITH_IPV6 {
        UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).await.ok()
    } else {
        None
    };

    let dht_v4 = socket_v4.map(|socket| {
        MainlineDht::builder()
            .add_routers(DHT_ROUTERS.iter().copied())
            .set_read_only(true)
            .start(socket)
            .unwrap()
    });

    let dht_v6 = socket_v6.map(|socket| {
        MainlineDht::builder()
            .add_routers(DHT_ROUTERS.iter().copied())
            .set_read_only(true)
            .start(socket)
            .unwrap()
    });

    let announce = match options.action {
        Action::Lookup => false,
        Action::Announce => true,
        Action::Single(action) => match action {
            Single::Lookup { .. } => false,
            Single::Announce { .. } => true,
        },
    };

    if options.token.is_some() && options.swarm_name.is_some() {
        println!("Only one of the token, swarm_name options may be set");
        return Ok(());
    }

    if let Some(info_hash) = info_hash {
        println!("InfoHash: {info_hash:?}");

        let task = task::spawn(async move {
            if let Some(dht) = dht_v4 {
                println!();
                lookup("IPv4", &dht, info_hash, announce).await;
            }

            if let Some(dht) = dht_v6 {
                println!();
                lookup("IPv6", &dht, info_hash, announce).await;
            }
        });

        task.await?;
    } else {
        // This never ends, useful mainly for debugging.
        std::future::pending::<()>().await;
    }

    Ok(())
}

async fn run_single_action(info_hash: InfoHash, command: &Single) {
    use btdht::message::{AnnouncePeerRequest, GetPeersRequest, Message, MessageBody, Request};
    use std::time::Duration;
    use tokio::time::timeout;

    let (node_addr, announce) = match command {
        Single::Lookup { peer } => (peer, false),
        Single::Announce { peer } => (peer, true),
    };

    let socket = match node_addr {
        SocketAddr::V4(_) => UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await.unwrap(),
        SocketAddr::V6(_) => UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).await.unwrap(),
    };

    let get_peers = Message {
        transaction_id: Vec::from(rand::random::<[u8; 4]>()),
        body: MessageBody::Request(Request::GetPeers(GetPeersRequest {
            id: rand::random(),
            info_hash,
            want: None,
        })),
    };

    println!("Sending GetPeers request to {node_addr:?}");
    socket
        .send_to(&get_peers.encode(), node_addr)
        .await
        .unwrap();

    let response = loop {
        let mut buffer = vec![0u8; 1500];

        let (size, _from) = timeout(Duration::from_secs(5), socket.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();

        buffer.truncate(size);
        let response = Message::decode(&buffer).unwrap();
        if response.transaction_id == get_peers.transaction_id {
            match response.body {
                MessageBody::Request(_) => {
                    println!("Received a request to request");
                }
                MessageBody::Response(response) => {
                    println!("Received {response:?}");
                    break response;
                }
                MessageBody::Error(error) => {
                    println!("Received error response: {error:?}");
                }
            }
            return;
        }
    };

    if announce {
        let announce = Message {
            transaction_id: Vec::from(rand::random::<[u8; 4]>()),
            body: MessageBody::Request(Request::AnnouncePeer(AnnouncePeerRequest {
                id: rand::random(),
                info_hash,
                token: response.token.clone().unwrap(),
                port: None,
            })),
        };

        socket.send_to(&announce.encode(), node_addr).await.unwrap();
    }
}

async fn lookup(prefix: &str, dht: &MainlineDht, info_hash: InfoHash, announce: bool) {
    println!("{prefix} Bootstrapping...");
    if dht.bootstrapped(None).await {
        let mut seen_peers = HashSet::new();

        if !announce {
            println!("{prefix} Searching for peers...");
        } else {
            println!("{prefix} Searching and announcing for peers...");
        }

        let mut peers = dht.search(info_hash, announce);

        while let Some(peer) = peers.next().await {
            if seen_peers.insert(peer) {
                println!("  {peer:?}");
            }
        }
    } else {
        println!("{prefix} Bootstrap failed")
    }
}
