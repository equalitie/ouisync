use super::{
    peer_addr::PeerAddr,
    quic,
    seen_peers::{SeenPeer, SeenPeers},
};
use crate::{
    scoped_task::{self, ScopedJoinHandle},
    state_monitor::StateMonitor,
};
use btdht::{InfoHash, MainlineDht};
use chrono::{offset::Local, DateTime};
use futures_util::{stream, StreamExt};
use rand::Rng;
use std::{
    borrow::Cow,
    collections::HashMap,
    future::pending,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Duration, SystemTime},
};
use tokio::{
    select,
    sync::{mpsc, watch},
    time,
};
use tracing::instrument::Instrument;

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

pub(super) struct DhtDiscovery {
    v4: Mutex<RestartableDht>,
    v6: Mutex<RestartableDht>,
    lookups: Arc<Mutex<Lookups>>,
    next_id: AtomicU64,
    monitor: StateMonitor,
}

impl DhtDiscovery {
    pub fn new(
        socket_maker_v4: Option<quic::SideChannelMaker>,
        socket_maker_v6: Option<quic::SideChannelMaker>,
        monitor: StateMonitor,
    ) -> Self {
        let v4 = Mutex::new(RestartableDht::new(socket_maker_v4));
        let v6 = Mutex::new(RestartableDht::new(socket_maker_v6));
        let lookups = Arc::new(Mutex::new(HashMap::default()));

        Self {
            v4,
            v6,
            lookups,
            next_id: AtomicU64::new(0),
            monitor,
        }
    }

    pub fn lookup(
        &self,
        info_hash: InfoHash,
        found_peers_tx: mpsc::UnboundedSender<SeenPeer>,
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
                let dht_v4 = self.v4.lock().unwrap().fetch(&self.monitor);
                let dht_v6 = self.v6.lock().unwrap().fetch(&self.monitor);

                Lookup::new(dht_v4, dht_v6, info_hash, &self.monitor)
            })
            .add_request(id, found_peers_tx);

        request
    }
}

// Wrapper for a DHT instance that can be stopped and restarted at any point.
struct RestartableDht {
    socket_maker: Option<quic::SideChannelMaker>,
    dht: Weak<Option<MonitoredDht>>,
}

impl RestartableDht {
    fn new(socket_maker: Option<quic::SideChannelMaker>) -> Self {
        Self {
            socket_maker,
            dht: Weak::new(),
        }
    }

    // Retrieve a shared pointer to a running DHT instance if there is one already or start a new
    // one. When all such pointers are dropped, the underlying DHT is terminated.
    fn fetch(&mut self, monitor: &StateMonitor) -> Arc<Option<MonitoredDht>> {
        if let Some(dht) = self.dht.upgrade() {
            dht
        } else if let Some(maker) = &self.socket_maker {
            let socket = maker.make();
            let dht = MonitoredDht::start(socket, monitor);
            let dht = Arc::new(Some(dht));

            self.dht = Arc::downgrade(&dht);

            dht
        } else {
            Arc::new(None)
        }
    }
}

// Wrapper for a DHT instance that periodically outputs it's state to the provided StateMonitor.
struct MonitoredDht {
    dht: MainlineDht,
    _monitoring_task: ScopedJoinHandle<()>,
}

impl MonitoredDht {
    fn start(socket: quic::SideChannel, monitor: &StateMonitor) -> Self {
        // TODO: Unwrap
        let local_addr = socket.local_addr().unwrap();

        let protocol = match local_addr {
            SocketAddr::V4(_) => "IPv4",
            SocketAddr::V6(_) => "IPv6",
        };

        let monitor = monitor.make_child(protocol);

        // TODO: load the DHT state from a previous save if it exists.
        let dht = MainlineDht::builder()
            .add_routers(DHT_ROUTERS.iter().copied())
            .set_read_only(false)
            .start(socket)
            // Unwrap OK because `start` only fails if it can't get `local_addr` out of the socket, but
            // since we just succeeded in binding the socket above, that shouldn't happen.
            .unwrap();

        // Spawn a task to monitor the DHT status.
        let monitoring_task = scoped_task::spawn({
            let dht = dht.clone();

            let first_bootstrap =
                monitor.make_value::<&'static str>("first_bootstrap".into(), "in progress");

            async move {
                if dht.bootstrapped(None).await {
                    *first_bootstrap.get() = "done";
                    tracing::info!("DHT {} bootstrap complete", protocol);
                } else {
                    *first_bootstrap.get() = "failed";
                    tracing::error!("DHT {} bootstrap failed", protocol);

                    // Don't `return`, instead halt here so that the `first_bootstrap` monitored value
                    // is preserved for the user to see.
                    pending::<()>().await;
                }

                let i = monitor.make_value::<u64>("probe_counter".into(), 0);

                let is_running = monitor.make_value::<Option<bool>>("is_running".into(), None);
                let bootstrapped = monitor.make_value::<Option<bool>>("bootstrapped".into(), None);
                let good_node_count =
                    monitor.make_value::<Option<usize>>("good_node_count".into(), None);
                let questionable_node_count =
                    monitor.make_value::<Option<usize>>("questionable_node_count".into(), None);
                let bucket_count = monitor.make_value::<Option<usize>>("bucket_count".into(), None);

                loop {
                    *i.get() += 1;

                    if let Some(state) = dht.get_state().await {
                        *is_running.get() = Some(state.is_running);
                        *bootstrapped.get() = Some(state.bootstrapped);
                        *good_node_count.get() = Some(state.good_node_count);
                        *questionable_node_count.get() = Some(state.questionable_node_count);
                        *bucket_count.get() = Some(state.bucket_count);
                    } else {
                        *is_running.get() = None;
                        *bootstrapped.get() = None;
                        *good_node_count.get() = None;
                        *questionable_node_count.get() = None;
                        *bucket_count.get() = None;
                    }
                    time::sleep(Duration::from_secs(5)).await;
                }
            }
        });

        Self {
            dht,
            _monitoring_task: monitoring_task,
        }
    }
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
    seen_peers: Arc<SeenPeers>,
    requests: Arc<Mutex<HashMap<RequestId, mpsc::UnboundedSender<SeenPeer>>>>,
    wake_up_tx: watch::Sender<()>,
    _task: ScopedJoinHandle<()>,
}

impl Lookup {
    fn new(
        dht_v4: Arc<Option<MonitoredDht>>,
        dht_v6: Arc<Option<MonitoredDht>>,
        info_hash: InfoHash,
        monitor: &StateMonitor,
    ) -> Self {
        let (wake_up_tx, mut wake_up_rx) = watch::channel(());
        // Mark the initial value as seen so the change notification is not triggered immediately
        // but only when we create the first request.
        wake_up_rx.borrow_and_update();

        let monitor = monitor
            .make_child("lookups")
            .make_child(format!("{:?}", info_hash));

        let seen_peers = Arc::new(SeenPeers::new());
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

    fn add_request(&mut self, id: RequestId, tx: mpsc::UnboundedSender<SeenPeer>) {
        for peer in self.seen_peers.collect() {
            tx.send(peer.clone()).unwrap_or(());
        }

        self.requests.lock().unwrap().insert(id, tx);
        self.wake_up_tx.send(()).unwrap();
    }

    fn start_task(
        dht_v4: Arc<Option<MonitoredDht>>,
        dht_v6: Arc<Option<MonitoredDht>>,
        info_hash: InfoHash,
        seen_peers: Arc<SeenPeers>,
        requests: Arc<Mutex<HashMap<RequestId, mpsc::UnboundedSender<SeenPeer>>>>,
        mut wake_up: watch::Receiver<()>,
        monitor: &StateMonitor,
    ) -> ScopedJoinHandle<()> {
        let state =
            monitor.make_value::<Cow<'static, str>>("state".into(), Cow::Borrowed("started"));

        let span = tracing::debug_span!("DHT search", ?info_hash);
        let task = async move {
            // Wait for the first request to be created
            wake_up.changed().await.unwrap_or(());

            loop {
                seen_peers.start_new_round();

                tracing::debug!("starting search");
                *state.get() = Cow::Borrowed("making request");

                // find peers for the repo and also announce that we have it.
                let dhts = dht_v4.iter().chain(dht_v6.iter());

                let mut peers = Box::pin(stream::iter(dhts).flat_map(|dht| {
                    stream::once(async {
                        dht.dht.bootstrapped(Some(Duration::from_secs(10))).await;
                        dht.dht.search(info_hash, true)
                    })
                    .flatten()
                }));

                *state.get() = Cow::Borrowed("awaiting results");

                while let Some(addr) = peers.next().await {
                    if let Some(peer) = seen_peers.insert(PeerAddr::Quic(addr)) {
                        tracing::debug!("found peer {:?}", peer.addr());

                        for tx in requests.lock().unwrap().values() {
                            tx.send(peer.clone()).unwrap_or(());
                        }
                    }
                }

                // sleep a random duration before the next search, but wake up if there is a new
                // request.
                let duration =
                    rand::thread_rng().gen_range(MIN_DHT_ANNOUNCE_DELAY..MAX_DHT_ANNOUNCE_DELAY);

                {
                    let time: DateTime<Local> = (SystemTime::now() + duration).into();
                    tracing::debug!(
                        "search ended. next one scheduled at {} (in {:?})",
                        time.format("%T"),
                        duration
                    );
                    *state.get() = Cow::Owned(format!("sleeping until {}", time.format("%T")));
                }

                select! {
                    _ = time::sleep(duration) => {
                        tracing::trace!("sleep duration passed")
                    },
                    _ = wake_up.changed() => {
                        tracing::trace!("wake up")
                    },
                }
            }
        };
        let task = task.instrument(span);

        scoped_task::spawn(task)
    }
}
