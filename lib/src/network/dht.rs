mod lookup_stream;
mod monitored;
mod restartable;

pub use lookup_stream::DhtLookupStream;

use super::{
    peer_addr::PeerAddr,
    seen_peers::{SeenPeer, SeenPeers},
};
use async_trait::async_trait;
use btdht::InfoHash;
use chrono::{DateTime, offset::Local};
use deadlock::{AsyncMutex, BlockingMutex};
use futures_util::{StreamExt, stream};
use monitored::MonitoredDht;
use net::quic;
use rand::Rng;
use restartable::RestartableDht;
use scoped_task::ScopedJoinHandle;
use state_monitor::StateMonitor;
use std::{
    collections::{HashMap, HashSet},
    io,
    net::{SocketAddrV4, SocketAddrV6},
    pin::pin,
    sync::{
        Arc, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};
use tokio::{
    select,
    sync::{mpsc, watch},
    time::{self, Duration, timeout},
};
use tracing::{Span, instrument::Instrument};

// Hardcoded DHT routers to bootstrap the DHT against.
pub const DEFAULT_DHT_ROUTERS: &[&str] = &[
    "dht.ouisync.net:6881",
    "router.bittorrent.com:6881",
    "dht.transmissionbt.com:6881",
];

// Interval for the delay before a repository is re-announced on the DHT. The actual delay is an
// uniformly random value from this interval.
// BEP5 indicates that "After 15 minutes of inactivity, a node becomes questionable." so try not
// to get too close to that value to avoid DHT churn. However, too frequent updates may cause
// other nodes to put us on a blacklist.
const MIN_DHT_ANNOUNCE_DELAY: Duration = Duration::from_secs(3 * 60);
const MAX_DHT_ANNOUNCE_DELAY: Duration = Duration::from_secs(6 * 60);

/// Persistent store for DHT contacts. It is periodically updated with the
/// current DHT contacts and persisted. It is then used to provide additional
/// nodes to bootstrap against on the next DHT discovery startup.
#[async_trait]
pub trait DhtContactsStoreTrait: Sync + Send + 'static {
    async fn load_v4(&self) -> io::Result<HashSet<SocketAddrV4>>;
    async fn load_v6(&self) -> io::Result<HashSet<SocketAddrV6>>;
    async fn store_v4(&self, contacts: HashSet<SocketAddrV4>) -> io::Result<()>;
    async fn store_v6(&self, contacts: HashSet<SocketAddrV6>) -> io::Result<()>;
}

pub(super) enum DhtEvent {
    PeerFound(SeenPeer),
    RoundEnded,
}

pub(super) struct DhtDiscovery {
    v4: BlockingMutex<RestartableDht>,
    v6: BlockingMutex<RestartableDht>,
    lookups: Arc<BlockingMutex<Lookups>>,
    next_id: AtomicU64,
    main_monitor: StateMonitor,
    lookups_monitor: StateMonitor,
    span: Span,
}

impl DhtDiscovery {
    pub fn new(
        socket_maker_v4: Option<quic::SideChannelMaker>,
        socket_maker_v6: Option<quic::SideChannelMaker>,
        contacts: Option<Arc<dyn DhtContactsStoreTrait>>,
        monitor: StateMonitor,
    ) -> Self {
        let routers: HashSet<String> = DEFAULT_DHT_ROUTERS
            .iter()
            .copied()
            .map(Into::into)
            .collect();

        let mut v4 = RestartableDht::new(socket_maker_v4, contacts.clone());
        v4.set_routers(routers.clone());

        let mut v6 = RestartableDht::new(socket_maker_v6, contacts);
        v6.set_routers(routers);

        let lookups = Arc::new(BlockingMutex::new(HashMap::default()));

        let lookups_monitor = monitor.make_child("lookups");

        Self {
            v4: BlockingMutex::new(v4),
            v6: BlockingMutex::new(v6),
            lookups,
            next_id: AtomicU64::new(0),
            span: Span::current(),
            main_monitor: monitor,
            lookups_monitor,
        }
    }

    /// Binds new sockets to the DHT instances, rebootstraps the DHTs and restarts any ongoing
    /// lookups.
    pub fn rebind(
        &self,
        socket_maker_v4: Option<quic::SideChannelMaker>,
        socket_maker_v6: Option<quic::SideChannelMaker>,
    ) {
        self.reset(|v4, v6| {
            v4.rebind(socket_maker_v4);
            v6.rebind(socket_maker_v6);
        })
    }

    /// Changes the DHT routers, rebootstraps the DHTs and restart any ongoing lookups.
    pub fn set_routers(&self, routers: HashSet<String>) {
        self.reset(|v4, v6| {
            v4.set_routers(routers.clone());
            v6.set_routers(routers);
        })
    }

    /// Returns the current DHT routers.
    pub fn routers(&self) -> HashSet<String> {
        self.v4
            .lock()
            .unwrap()
            .routers()
            .iter()
            .chain(self.v6.lock().unwrap().routers())
            .cloned()
            .collect()
    }

    pub fn start_lookup(
        &self,
        info_hash: InfoHash,
        announce: bool,
        event_tx: mpsc::UnboundedSender<DhtEvent>,
    ) -> LookupRequest {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        self.lookups
            .lock()
            .unwrap()
            .entry(info_hash)
            .or_insert_with(|| {
                let dht_v4 = self
                    .v4
                    .lock()
                    .unwrap()
                    .fetch(&self.main_monitor, &self.span);

                let dht_v6 = self
                    .v6
                    .lock()
                    .unwrap()
                    .fetch(&self.main_monitor, &self.span);

                Lookup::start(dht_v4, dht_v6, info_hash, &self.lookups_monitor, &self.span)
            })
            .add_request(id, announce, event_tx);

        LookupRequest {
            id,
            info_hash,
            lookups: Arc::downgrade(&self.lookups),
        }
    }

    fn reset<F>(&self, f: F)
    where
        F: FnOnce(&mut RestartableDht, &mut RestartableDht),
    {
        let mut v4 = self.v4.lock().unwrap();
        let mut v6 = self.v6.lock().unwrap();

        f(&mut v4, &mut v6);

        let mut lookups = self.lookups.lock().unwrap();

        if lookups.is_empty() {
            return;
        }

        let dht_v4 = v4.fetch(&self.main_monitor, &self.span);
        let dht_v6 = v6.fetch(&self.main_monitor, &self.span);

        for (info_hash, lookup) in &mut *lookups {
            lookup.restart(
                dht_v4.clone(),
                dht_v6.clone(),
                *info_hash,
                &self.lookups_monitor,
                &self.span,
            );
        }
    }
}

type Lookups = HashMap<InfoHash, Lookup>;

type RequestId = u64;

pub struct LookupRequest {
    id: RequestId,
    info_hash: InfoHash,
    lookups: Weak<BlockingMutex<Lookups>>,
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
    requests: Arc<BlockingMutex<HashMap<RequestId, RequestData>>>,
    wake_up_tx: watch::Sender<()>,
    task: Option<ScopedJoinHandle<()>>,
}

struct RequestData {
    event_tx: mpsc::UnboundedSender<DhtEvent>,
    announce: bool,
}

impl Lookup {
    fn start(
        dht_v4: Option<Arc<TaskOrResult<MonitoredDht>>>,
        dht_v6: Option<Arc<TaskOrResult<MonitoredDht>>>,
        info_hash: InfoHash,
        monitor: &StateMonitor,
        span: &Span,
    ) -> Self {
        let (wake_up_tx, mut wake_up_rx) = watch::channel(());
        // Mark the initial value as seen so the change notification is not triggered immediately
        // but only when we create the first request.
        wake_up_rx.borrow_and_update();

        let seen_peers = Arc::new(SeenPeers::new());
        let requests = Arc::new(BlockingMutex::new(HashMap::default()));

        let task = if dht_v4.is_some() || dht_v6.is_some() {
            Some(Self::start_task(
                dht_v4,
                dht_v6,
                info_hash,
                seen_peers.clone(),
                requests.clone(),
                wake_up_rx,
                monitor,
                span,
            ))
        } else {
            None
        };

        Self {
            seen_peers,
            requests,
            wake_up_tx,
            task,
        }
    }

    // Start this same lookup on different DHT instances
    fn restart(
        &mut self,
        dht_v4: Option<Arc<TaskOrResult<MonitoredDht>>>,
        dht_v6: Option<Arc<TaskOrResult<MonitoredDht>>>,
        info_hash: InfoHash,
        monitor: &StateMonitor,
        span: &Span,
    ) {
        if dht_v4.is_none() && dht_v6.is_none() {
            self.task.take();
            return;
        }

        let task = Self::start_task(
            dht_v4,
            dht_v6,
            info_hash,
            self.seen_peers.clone(),
            self.requests.clone(),
            self.wake_up_tx.subscribe(),
            monitor,
            span,
        );

        self.task = Some(task);
        self.wake_up_tx.send(()).ok();
    }

    fn add_request(
        &mut self,
        id: RequestId,
        announce: bool,
        event_tx: mpsc::UnboundedSender<DhtEvent>,
    ) {
        self.requests
            .lock()
            .unwrap()
            .insert(id, RequestData { event_tx, announce });
        // `unwrap_or` because if the network is down, there should be no tasks that listen to this
        // wake up request.
        self.wake_up_tx.send(()).unwrap_or(());
    }

    #[allow(clippy::too_many_arguments)]
    fn start_task(
        dht_v4: Option<Arc<TaskOrResult<MonitoredDht>>>,
        dht_v6: Option<Arc<TaskOrResult<MonitoredDht>>>,
        info_hash: InfoHash,
        seen_peers: Arc<SeenPeers>,
        requests: Arc<BlockingMutex<HashMap<RequestId, RequestData>>>,
        mut wake_up: watch::Receiver<()>,
        lookups_monitor: &StateMonitor,
        span: &Span,
    ) -> ScopedJoinHandle<()> {
        let monitor = lookups_monitor.make_child(format!("{info_hash:?}"));
        let state = monitor.make_value("state", "started");
        let next = monitor.make_value("next", SystemTime::now().into());

        let task = async move {
            let dht_v4 = match &dht_v4 {
                Some(dht) => Some(dht.result().await),
                None => None,
            };

            let dht_v6 = match &dht_v6 {
                Some(dht) => Some(dht.result().await),
                None => None,
            };

            // Wait for the first request to be created
            wake_up.changed().await.unwrap_or(());

            loop {
                seen_peers.start_new_round();

                tracing::debug!(?info_hash, "starting search");
                *state.get() = "making request";

                // If at least one request wants to announce, we do announce. Otherwise we do only
                // lookup.
                let announce = requests
                    .lock()
                    .unwrap()
                    .values()
                    .any(|request| request.announce);

                let dhts = dht_v4.iter().chain(dht_v6.iter());
                let peers = stream::iter(dhts).flat_map(|dht| {
                    stream::once(async {
                        timeout(Duration::from_secs(10), dht.dht.bootstrapped())
                            .await
                            .unwrap_or(false);
                        dht.dht.search(info_hash, announce)
                    })
                    .flatten()
                });
                let mut peers = pin!(peers);

                *state.get() = "awaiting results";

                while let Some(addr) = peers.next().await {
                    if let Some(peer) = seen_peers.insert(PeerAddr::Quic(addr)) {
                        for request in requests.lock().unwrap().values() {
                            request
                                .event_tx
                                .send(DhtEvent::PeerFound(peer.clone()))
                                .ok();
                        }
                    }
                }

                for request in requests.lock().unwrap().values() {
                    request.event_tx.send(DhtEvent::RoundEnded).ok();
                }

                // sleep a random duration before the next search, but wake up if there is a new
                // request.
                let duration =
                    rand::thread_rng().gen_range(MIN_DHT_ANNOUNCE_DELAY..MAX_DHT_ANNOUNCE_DELAY);

                {
                    let time: DateTime<Local> = (SystemTime::now() + duration).into();
                    tracing::debug!(
                        ?info_hash,
                        "search ended. next one scheduled at {} (in {:?})",
                        time.format("%T"),
                        duration
                    );

                    *state.get() = "sleeping";
                    *next.get() = time;
                }

                select! {
                    _ = time::sleep(duration) => (),
                    _ = wake_up.changed() => (),
                }
            }
        };
        let task = task.instrument(span.clone());

        scoped_task::spawn(task)
    }
}

struct TaskOrResult<T> {
    task: AsyncMutex<Option<ScopedJoinHandle<T>>>,
    result: OnceLock<T>,
}

impl<T> TaskOrResult<T> {
    fn new(task: ScopedJoinHandle<T>) -> Self {
        Self {
            task: AsyncMutex::new(Some(task)),
            result: OnceLock::new(),
        }
    }

    // Note that this function is not cancel safe.
    async fn result(&self) -> &T {
        if let Some(result) = self.result.get() {
            return result;
        }

        let mut lock = self.task.lock().await;

        if let Some(handle) = lock.take() {
            // The unwrap is OK for the same reason we unwrap `BlockingMutex::lock()`s.
            // The assert is OK because we can await on the handle only once.
            assert!(self.result.set(handle.await.unwrap()).is_ok());
        }

        // Unwrap is OK because we ensured the `result` holds a value.
        self.result.get().unwrap()
    }
}
