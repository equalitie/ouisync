use super::socket;
use crate::{
    config::{ConfigKey, ConfigStore},
    scoped_task::{self, ScopedJoinHandle},
    state_monitor::StateMonitor,
};
use btdht::{InfoHash, IpVersion, MainlineDht};
use chrono::{offset::Local, DateTime};
use futures_util::{stream, StreamExt};
use rand::Rng;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    future::pending,
    io,
    net::{Ipv4Addr, Ipv6Addr, IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, RwLock, Weak,
    },
    time::{Duration, SystemTime},
};
use tokio::{
    net::UdpSocket,
    select,
    sync::{mpsc, watch},
    time,
};

// Hardcoded DHT routers to bootstrap the DHT against.
// TODO: add this to `NetworkOptions` so it can be overriden by the user.
pub const DHT_ROUTERS: &[&str] = &["router.bittorrent.com:6881", "dht.transmissionbt.com:6881"];

// Interval for the delay before a repository is re-announced on the DHT. The actual delay is an
// uniformly random value from this interval.
// BEP5 indicates that "After 15 minutes of inactivity, a node becomes questionable." so try not
// to get too close to that value to avoid DHT churn. However, too frequent updates may cause
// other nodes to put us on a blacklist.
pub const MIN_DHT_ANNOUNCE_DELAY: Duration = Duration::from_secs(3 * 60);
pub const MAX_DHT_ANNOUNCE_DELAY: Duration = Duration::from_secs(6 * 60);

const LAST_USED_PORT_V4: ConfigKey<u16> =
    ConfigKey::new("last_used_udp_port_v4", LAST_USED_PORT_COMMENT);

const LAST_USED_PORT_V6: ConfigKey<u16> =
    ConfigKey::new("last_used_udp_port_v6", LAST_USED_PORT_COMMENT);

const LAST_USED_PORT_COMMENT: &str =
    "The value stored in this file is the last used UDP port for listening on incoming\n\
     connections. It is used to avoid binding to a random port every time the application starts.\n\
     This, in turn, is mainly useful for users who can't or don't want to use UPnP and have to\n\
     default to manually setting up port forwarding on their routers.";

pub(super) struct DhtDiscovery {
    dht_v4: RestartableDht,
    dht_v6: RestartableDht,
    lookups: Arc<Mutex<Lookups>>,
    next_id: AtomicU64,
    monitor: Arc<StateMonitor>,
}

impl DhtDiscovery {
    pub async fn new(acceptor_port: u16, config: &ConfigStore, monitor: Arc<StateMonitor>) -> Self {
        let dht_v4 = start_dht(IpVersion::V4, acceptor_port, config, &monitor).await;
        let dht_v6 = start_dht(IpVersion::V6, acceptor_port, config, &monitor).await;

        let lookups = Arc::new(Mutex::new(HashMap::default()));

        Self {
            dht_v4,
            dht_v6,
            lookups,
            next_id: AtomicU64::new(0),
            monitor,
        }
    }

    pub fn lookup(
        &self,
        info_hash: InfoHash,
        found_peers_tx: mpsc::UnboundedSender<SocketAddr>,
    ) -> LookupRequest {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = LookupRequest {
            id,
            info_hash,
            lookups: Arc::downgrade(&self.lookups),
        };

        self.lookups
            .lock()
            .unwrap()
            .entry(info_hash)
            .or_insert_with(|| {
                Lookup::new(
                    self.dht_v4.clone(),
                    self.dht_v6.clone(),
                    info_hash,
                    &self.monitor,
                )
            })
            .add_request(id, found_peers_tx);

        request
    }

    pub fn local_addr_v4(&self) -> Option<&SocketAddr> {
        Some(&self.dht_v4.local_addr)
    }

    pub fn local_addr_v6(&self) -> Option<&SocketAddr> {
        Some(&self.dht_v6.local_addr)
    }
}

async fn start_dht(
    ip_v: IpVersion,
    acceptor_port: u16,
    config: &ConfigStore,
    monitor: &Arc<StateMonitor>,
) -> RestartableDht {
    // TODO: Unwrap
    let socket = bind(ip_v, config).await.unwrap();

    // Unwrap OK because `Socket::new` only fails if it can't get `local_addr` out of it, but since
    // we just succeeded in binding the socket above, that shouldn't happen.
    let socket = btdht::Socket::new(socket).unwrap();

    let local_addr = socket.local_addr();

    let protocol = match ip_v {
        IpVersion::V4 => "IPv4",
        IpVersion::V6 => "IPv6",
    };

    let monitor = monitor.make_child(protocol);

    // TODO: load the DHT state from a previous save if it exists.
    // TODO: Unwrap
    let dht = MainlineDht::builder()
        .add_routers(DHT_ROUTERS.iter().copied())
        .set_read_only(false)
        .set_announce_port(acceptor_port)
        .start(socket);

    // Spawn a task to monitor the DHT status.
    let monitoring_task = scoped_task::spawn({
        let dht = dht.clone();

        let bootstrap_state =
            monitor.make_value::<&'static str>("bootstrap_state".into(), "bootstrapping");

        async move {
            if dht.bootstrapped().await {
                *bootstrap_state.get() = "bootstrapped";
                log::info!("DHT {} bootstrap complete", protocol);
            } else {
                *bootstrap_state.get() = "bootstrap failed";
                log::error!("DHT {} bootstrap failed", protocol);

                // Don't `return`, instead halt here so that the `bootstrap_state` monitored value
                // is preserved for the user to see.
                pending::<()>().await;
            }

            let i = monitor.make_value::<u64>("probe_counter".into(), 0);

            let is_running = monitor.make_value::<Option<bool>>("is_running".into(), None);
            let good_node_count =
                monitor.make_value::<Option<usize>>("good_node_count".into(), None);
            let questionable_node_count =
                monitor.make_value::<Option<usize>>("questionable_node_count".into(), None);
            let bucket_count = monitor.make_value::<Option<usize>>("bucket_count".into(), None);

            loop {
                *i.get() += 1;

                if let Some(state) = dht.get_state().await {
                    *is_running.get() = Some(state.is_running);
                    *good_node_count.get() = Some(state.good_node_count);
                    *questionable_node_count.get() = Some(state.questionable_node_count);
                    *bucket_count.get() = Some(state.bucket_count);
                } else {
                    *is_running.get() = None;
                    *good_node_count.get() = None;
                    *questionable_node_count.get() = None;
                    *bucket_count.get() = None;
                }
                time::sleep(Duration::from_secs(5)).await;
            }
        }
    });

    RestartableDht {
        dht,
        local_addr,
        _monitoring_task: Arc::new(monitoring_task),
    }
}

// A wrapper over MainlineDht that periodically retries to initialize and bootstrap (and
// rebootstrap when/if all nodes disappear). This is necessary because the network may go up and
// down.
#[derive(Clone)]
struct RestartableDht {
    dht: MainlineDht,
    local_addr: SocketAddr,
    _monitoring_task: Arc<ScopedJoinHandle<()>>,
}

type Lookups = HashMap<InfoHash, Lookup>;

type RequestId = u64;

pub struct LookupRequest {
    id: RequestId,
    info_hash: InfoHash,
    lookups: Weak<Mutex<Lookups>>,
}

impl Drop for LookupRequest {
    fn drop(&mut self) {
        if let Some(lookups) = self.lookups.upgrade() {
            let mut lookups = lookups.lock().unwrap();

            let empty = if let Some(lookup) = lookups.get_mut(&self.info_hash) {
                let mut requests = lookup.requests.lock().unwrap();
                requests.remove(&self.id);
                requests.is_empty()
            } else {
                false
            };

            if empty {
                lookups.remove(&self.info_hash);
            }
        }
    }
}

struct Lookup {
    seen_peers: Arc<RwLock<HashSet<SocketAddr>>>, // To avoid duplicates
    requests: Arc<Mutex<HashMap<RequestId, mpsc::UnboundedSender<SocketAddr>>>>,
    wake_up_tx: watch::Sender<()>,
    _task: ScopedJoinHandle<()>,
}

impl Lookup {
    fn new(
        dht_v4: RestartableDht,
        dht_v6: RestartableDht,
        info_hash: InfoHash,
        monitor: &Arc<StateMonitor>,
    ) -> Self {
        let (wake_up_tx, mut wake_up_rx) = watch::channel(());
        // Mark the initial value as seen so the change notification is not triggered immediately
        // but only when we create the first request.
        wake_up_rx.borrow_and_update();

        let monitor = monitor
            .make_child("lookups")
            .make_child(format!("{:?}", info_hash));

        let seen_peers = Arc::new(RwLock::new(Default::default()));
        let requests = Arc::new(Mutex::new(HashMap::new()));

        let task = Self::start_task(
            dht_v4,
            dht_v6,
            info_hash,
            seen_peers.clone(),
            requests.clone(),
            wake_up_rx,
            &monitor,
        );

        Lookup {
            seen_peers,
            requests,
            wake_up_tx,
            _task: task,
        }
    }

    fn add_request(&mut self, id: RequestId, tx: mpsc::UnboundedSender<SocketAddr>) {
        for peer in self.seen_peers.read().unwrap().iter() {
            tx.send(*peer).unwrap_or(());
        }

        self.requests.lock().unwrap().insert(id, tx);
        self.wake_up_tx.send(()).unwrap();
    }

    fn start_task(
        dht_v4: RestartableDht,
        dht_v6: RestartableDht,
        info_hash: InfoHash,
        seen_peers: Arc<RwLock<HashSet<SocketAddr>>>,
        requests: Arc<Mutex<HashMap<RequestId, mpsc::UnboundedSender<SocketAddr>>>>,
        mut wake_up: watch::Receiver<()>,
        monitor: &Arc<StateMonitor>,
    ) -> ScopedJoinHandle<()> {
        let state =
            monitor.make_value::<Cow<'static, str>>("state".into(), Cow::Borrowed("started"));

        scoped_task::spawn(async move {
            // Wait for the first request to be created
            wake_up.changed().await.unwrap_or(());

            loop {
                log::debug!("DHT Starting search for info hash: {:?}", info_hash);
                *state.get() = Cow::Borrowed("making request");

                // find peers for the repo and also announce that we have it.
                let dhts = [&dht_v4, &dht_v6];

                let mut peers =
                    stream::iter(dhts.iter()).flat_map(|dht| dht.dht.search(info_hash, true));

                *state.get() = Cow::Borrowed("awaiting results");
                while let Some(peer) = peers.next().await {
                    if seen_peers.write().unwrap().insert(peer) {
                        log::debug!("DHT found peer for {:?}: {}", info_hash, peer);

                        for tx in requests.lock().unwrap().values() {
                            tx.send(peer).unwrap_or(());
                        }
                    }
                }

                seen_peers.write().unwrap().clear();

                // sleep a random duration before the next search, but wake up if there is a new
                // request.
                let duration =
                    rand::thread_rng().gen_range(MIN_DHT_ANNOUNCE_DELAY..MAX_DHT_ANNOUNCE_DELAY);

                {
                    let time: DateTime<Local> = (SystemTime::now() + duration).into();
                    log::debug!(
                        "DHT search for {:?} ended. Next one scheduled at {} (in {:?})",
                        info_hash,
                        time.format("%T"),
                        duration
                    );
                    *state.get() = Cow::Owned(format!("sleeping until {}", time.format("%T")));
                }

                select! {
                    _ = time::sleep(duration) => {
                        log::debug!("DHT sleep duration passed")
                    },
                    _ = wake_up.changed() => {
                        log::debug!("DHT wake up")
                    },
                }
            }
        })
    }
}

async fn bind(ip_v: IpVersion, config: &ConfigStore) -> io::Result<UdpSocket> {
    match ip_v {
        IpVersion::V4 => {
            socket::bind(
                (Ipv4Addr::UNSPECIFIED, 0).into(),
                config.entry(LAST_USED_PORT_V4),
            )
            .await
        }
        IpVersion::V6 => {
            let port = config.entry(LAST_USED_PORT_V6);

            // [BEP-32](https://www.bittorrent.org/beps/bep_0032.html) says we should bind the ipv6
            // socket to a concrete unicast address, not to an unspecified one.
            match local_ipv6_address().await {
                Some(addr) => {
                    match socket::bind((addr, 0).into(), port.clone()).await {
                        Ok(socket) => return Ok(socket),
                        Err(_) => (),
                    }
                },
                None => (),
            }

            socket::bind((Ipv6Addr::UNSPECIFIED, 0).into(), port).await
        }
    }
}

pub async fn local_ipv6_address() -> Option<IpAddr> {
    let socket = match UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).await {
        Ok(s) => s,
        Err(_) => return None,
    };

    // We're not actually connecting to it. It's only being used to let the system chose interface
    // that can connect to the outside internet.
    let google_public_dns = "[2001:4860:4860::8888]:8888".parse::<std::net::SocketAddr>().ok()?;

    match socket.connect(google_public_dns).await {
        Ok(()) => (),
        Err(_) => return None,
    };

    match socket.local_addr() {
        Ok(addr) => return Some(addr.ip()),
        Err(_) => return None,
    };
}
