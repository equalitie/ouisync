use crate::scoped_task::ScopedJoinHandle;
use btdht::{DhtEvent, InfoHash, MainlineDht};

use futures_util::future;
use rand::Rng;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, RwLock, Weak,
    },
    time::Duration,
};
use tokio::{
    net::{self, UdpSocket},
    select,
    sync::{mpsc, watch},
    task, time,
};

// Hardcoded DHT routers to bootstrap the DHT against.
// TODO: add this to `NetworkOptions` so it can be overriden by the user.
const DHT_ROUTERS: &[&str] = &["router.bittorrent.com:6881", "dht.transmissionbt.com:6881"];

// Interval for the delay before a repository is re-announced on the DHT. The actual delay is an
// uniformly random value from this interval.
// BEP5 indicatest that "After 15 minutes of inactivity, a node becomes questionable." so try not
// to get too close to that value to avoid DHT churn.
const MIN_DHT_ANNOUNCE_DELAY: Duration = Duration::from_secs(5 * 60);
const MAX_DHT_ANNOUNCE_DELAY: Duration = Duration::from_secs(12 * 60);

pub struct DhtDiscovery {
    dht: MainlineDht,
    lookups: Arc<Mutex<Lookups>>,
    next_id: AtomicU64,
    _dht_event_handler_task: ScopedJoinHandle<()>,
}

impl DhtDiscovery {
    pub async fn new(dht_socket: UdpSocket, acceptor_port: u16) -> Self {
        // TODO: load the DHT state from a previous save if it exists.
        let (dht, event_rx) = MainlineDht::builder()
            .add_routers(dht_router_addresses().await)
            .set_read_only(false)
            .set_announce_port(acceptor_port)
            .start(dht_socket);

        let lookups = Arc::new(Mutex::new(HashMap::default()));

        let dht_event_handler_task = ScopedJoinHandle(task::spawn({
            let lookups = lookups.clone();
            async move {
                Self::run_dht_event_handler(event_rx, lookups).await;
            }
        }));

        Self {
            dht,
            lookups,
            next_id: AtomicU64::new(0),
            _dht_event_handler_task: dht_event_handler_task,
        }
    }

    pub fn lookup(
        &self,
        info_hash: InfoHash,
        found_peers_tx: mpsc::UnboundedSender<SocketAddr>,
    ) -> Arc<LookupRequest> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = Arc::new(LookupRequest {
            id,
            info_hash,
            lookups: Arc::downgrade(&self.lookups),
            found_peers_tx: found_peers_tx,
        });

        self.lookups
            .lock()
            .unwrap()
            .entry(info_hash)
            .or_insert_with(|| Lookup::new(self.dht.clone(), info_hash))
            .add_request(id, &request);

        request
    }

    async fn run_dht_event_handler(
        mut event_rx: mpsc::UnboundedReceiver<DhtEvent>,
        lookups: Arc<Mutex<Lookups>>,
    ) {
        while let Some(event) = event_rx.recv().await {
            match event {
                DhtEvent::BootstrapCompleted => log::info!("DHT bootstrap complete"),
                DhtEvent::BootstrapFailed => {
                    log::error!("DHT bootstrap failed");
                    break;
                }
                DhtEvent::PeerFound(info_hash, addr) => {
                    log::debug!("DHT found peer for {:?}: {}", info_hash, addr);
                    let mut lookups = lookups.lock().unwrap();

                    if let Some(lookup) = lookups.get_mut(&info_hash) {
                        if lookup.seen_peers.write().unwrap().insert(addr) {
                            for request in lookup.requests.values() {
                                request.upgrade().map(|rq| rq.on_peer_found(addr));
                            }
                        }
                    }
                }
                DhtEvent::LookupCompleted(info_hash) => {
                    log::debug!("DHT lookup for {:?} complete", info_hash);
                    let mut lookups = lookups.lock().unwrap();

                    if let Some(lookup) = lookups.get_mut(&info_hash) {
                        let _ = lookup.iteration_finished_tx.send(());
                    }
                }
            }
        }
    }
}

type Lookups = HashMap<InfoHash, Lookup>;

type RequestId = u64;

pub struct LookupRequest {
    id: RequestId,
    info_hash: InfoHash,
    lookups: Weak<Mutex<Lookups>>,
    found_peers_tx: mpsc::UnboundedSender<SocketAddr>,
}

impl LookupRequest {
    fn on_peer_found(&self, addr: SocketAddr) {
        let _ = self.found_peers_tx.send(addr);
    }
}

impl Drop for LookupRequest {
    fn drop(&mut self) {
        if let Some(lookups) = self.lookups.upgrade() {
            let mut lookups = lookups.lock().unwrap();

            let empty = if let Some(lookup) = lookups.get_mut(&self.info_hash) {
                lookup.requests.remove(&self.id);
                lookup.requests.is_empty()
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
    info_hash: InfoHash,
    seen_peers: Arc<RwLock<HashSet<SocketAddr>>>, // To avoid duplicates
    requests: HashMap<RequestId, Weak<LookupRequest>>,
    wake_up_tx: watch::Sender<()>,
    iteration_finished_tx: watch::Sender<()>,
    _task: ScopedJoinHandle<()>,
}

impl Lookup {
    fn new(dht: MainlineDht, info_hash: InfoHash) -> Self {
        let (wake_up_tx, wake_up_rx) = watch::channel(());
        let (iteration_finished_tx, iteration_finished_rx) = watch::channel(());

        let seen_peers = Arc::new(RwLock::new(Default::default()));

        Lookup {
            info_hash,
            seen_peers: seen_peers.clone(),
            requests: Default::default(),
            wake_up_tx,
            iteration_finished_tx,
            _task: Self::start_task(
                dht,
                info_hash,
                seen_peers,
                wake_up_rx,
                iteration_finished_rx,
            ),
        }
    }

    fn add_request(&mut self, id: RequestId, request: &Arc<LookupRequest>) {
        assert_eq!(self.info_hash, request.info_hash);

        self.requests.insert(id, Arc::downgrade(&request));

        for peer in self.seen_peers.read().unwrap().iter() {
            request.on_peer_found(*peer);
        }

        self.wake_up_tx.send(()).unwrap();
    }

    fn start_task(
        dht: MainlineDht,
        info_hash: InfoHash,
        seen_peers: Arc<RwLock<HashSet<SocketAddr>>>,
        mut wake_up: watch::Receiver<()>,
        mut iteration_finished: watch::Receiver<()>,
    ) -> ScopedJoinHandle<()> {
        ScopedJoinHandle(task::spawn(async move {
            loop {
                // find peers for the repo and also announce that we have it.
                dht.search(info_hash, true);

                // Shouldn't fail because if the Sender is Dropped, this task should have been
                // Dropped as well.
                iteration_finished.changed().await.unwrap();

                seen_peers.write().unwrap().clear();

                // sleep a random duration before the next search, but wake up if there is a new
                // request.
                let duration =
                    rand::thread_rng().gen_range(MIN_DHT_ANNOUNCE_DELAY..MAX_DHT_ANNOUNCE_DELAY);

                select! {
                    _ = time::sleep(duration) => (),
                    r = wake_up.changed() => {
                        // Shouldn't fail because if the sender is Dropped, this task shold have
                        // been Dropped as well.
                        r.unwrap()
                    },
                }
            }
        }))
    }
}

async fn dht_router_addresses() -> Vec<SocketAddr> {
    future::join_all(DHT_ROUTERS.iter().map(net::lookup_host))
        .await
        .into_iter()
        .filter_map(|result| result.ok())
        .flatten()
        .collect()
}
