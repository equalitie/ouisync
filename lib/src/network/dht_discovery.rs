use super::{
    peer_addr::PeerAddr,
    seen_peers::{SeenPeer, SeenPeers},
};
use crate::{
    collections::{hash_map, HashMap, HashSet},
    deadlock::{AsyncMutex, BlockingMutex},
    state_monitor::StateMonitor,
};
use async_trait::async_trait;
use btdht::{InfoHash, MainlineDht};
use chrono::{offset::Local, DateTime};
use futures_util::{stream, StreamExt};
use net::quic;
use rand::Rng;
use scoped_task::ScopedJoinHandle;
use std::{
    future::pending,
    io,
    net::{SocketAddr, SocketAddrV4, SocketAddrV6},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Weak,
    },
    time::SystemTime,
};
use tokio::{
    select,
    sync::{mpsc, watch},
    time::{self, Duration},
};
use tracing::{instrument::Instrument, Span};

// Hardcoded DHT routers to bootstrap the DHT against.
// TODO: add this to `NetworkOptions` so it can be overriden by the user.
pub const DHT_ROUTERS: &[&str] = &[
    "dht.ouisync.net:6881",
    "router.bittorrent.com:6881",
    "dht.transmissionbt.com:6881",
];

// Interval for the delay before a repository is re-announced on the DHT. The actual delay is an
// uniformly random value from this interval.
// BEP5 indicates that "After 15 minutes of inactivity, a node becomes questionable." so try not
// to get too close to that value to avoid DHT churn. However, too frequent updates may cause
// other nodes to put us on a blacklist.
pub const MIN_DHT_ANNOUNCE_DELAY: Duration = Duration::from_secs(3 * 60);
pub const MAX_DHT_ANNOUNCE_DELAY: Duration = Duration::from_secs(6 * 60);

#[derive(Clone)]
pub struct ActiveDhtNodes {
    pub good: HashSet<SocketAddr>,
    pub questionable: HashSet<SocketAddr>,
}

#[async_trait]
pub trait DhtContactsStoreTrait: Sync + Send + 'static {
    async fn load_v4(&self) -> io::Result<HashSet<SocketAddrV4>>;
    async fn load_v6(&self) -> io::Result<HashSet<SocketAddrV6>>;
    async fn store_v4(&self, contacts: HashSet<SocketAddrV4>) -> io::Result<()>;
    async fn store_v6(&self, contacts: HashSet<SocketAddrV6>) -> io::Result<()>;
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
        contacts_store: Option<Arc<dyn DhtContactsStoreTrait>>,
        monitor: StateMonitor,
    ) -> Self {
        let v4 = BlockingMutex::new(RestartableDht::new(socket_maker_v4, contacts_store.clone()));
        let v6 = BlockingMutex::new(RestartableDht::new(socket_maker_v6, contacts_store));

        let lookups = Arc::new(BlockingMutex::new(HashMap::default()));

        let lookups_monitor = monitor.make_child("lookups");

        Self {
            v4,
            v6,
            lookups,
            next_id: AtomicU64::new(0),
            span: Span::current(),
            main_monitor: monitor,
            lookups_monitor,
        }
    }

    // Bind new sockets to the DHT instances. If there are any ongoing lookups, the current DHTs
    // are terminated, new DHTs with the new sockets are created and the lookups are restarted on
    // those new DHTs.
    pub fn rebind(
        &self,
        socket_maker_v4: Option<quic::SideChannelMaker>,
        socket_maker_v6: Option<quic::SideChannelMaker>,
    ) {
        let mut v4 = self.v4.lock().unwrap();
        let mut v6 = self.v6.lock().unwrap();

        v4.rebind(socket_maker_v4);
        v6.rebind(socket_maker_v6);

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

    pub fn start_lookup(
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

        let mut lookups = self.lookups.lock().unwrap();

        match lookups.entry(info_hash) {
            hash_map::Entry::Occupied(mut entry) => entry.get_mut().add_request(id, found_peers_tx),
            hash_map::Entry::Vacant(entry) => {
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

                entry
                    .insert(Lookup::start(
                        dht_v4,
                        dht_v6,
                        info_hash,
                        &self.lookups_monitor,
                        &self.span,
                    ))
                    .add_request(id, found_peers_tx);
            }
        }

        request
    }
}

// Wrapper for a DHT instance that can be stopped and restarted at any point.
struct RestartableDht {
    socket_maker: Option<quic::SideChannelMaker>,
    dht: Weak<Option<TaskOrResult<MonitoredDht>>>,
    contacts_store: Option<Arc<dyn DhtContactsStoreTrait>>,
}

impl RestartableDht {
    fn new(
        socket_maker: Option<quic::SideChannelMaker>,
        contacts_store: Option<Arc<dyn DhtContactsStoreTrait>>,
    ) -> Self {
        Self {
            socket_maker,
            dht: Weak::new(),
            contacts_store,
        }
    }

    // Retrieve a shared pointer to a running DHT instance if there is one already or start a new
    // one. When all such pointers are dropped, the underlying DHT is terminated.
    fn fetch(
        &mut self,
        monitor: &StateMonitor,
        span: &Span,
    ) -> Arc<Option<TaskOrResult<MonitoredDht>>> {
        if let Some(dht) = self.dht.upgrade() {
            dht
        } else if let Some(maker) = &self.socket_maker {
            let socket = maker.make();
            let dht = MonitoredDht::start(socket, monitor, span, self.contacts_store.clone());

            let dht = Arc::new(Some(dht));

            self.dht = Arc::downgrade(&dht);

            dht
        } else {
            Arc::new(None)
        }
    }

    fn rebind(&mut self, socket_maker: Option<quic::SideChannelMaker>) {
        self.socket_maker = socket_maker;
        self.dht = Weak::new();
    }
}

// Wrapper for a DHT instance that periodically outputs it's state to the provided StateMonitor.
struct MonitoredDht {
    dht: MainlineDht,
    _monitoring_task: ScopedJoinHandle<()>,
    _periodic_dht_node_load_task: Option<ScopedJoinHandle<()>>,
}

impl MonitoredDht {
    fn start(
        socket: quic::SideChannel,
        parent_monitor: &StateMonitor,
        span: &Span,
        contacts_store: Option<Arc<dyn DhtContactsStoreTrait>>,
    ) -> TaskOrResult<Self> {
        // TODO: Unwrap
        let local_addr = socket.local_addr().unwrap();

        let (is_v4, monitor_name, span) = match local_addr {
            SocketAddr::V4(_) => (true, "IPv4", tracing::info_span!(parent: span, "DHT/IPv4")),
            SocketAddr::V6(_) => (false, "IPv6", tracing::info_span!(parent: span, "DHT/IPv6")),
        };

        let monitor = parent_monitor.make_child(monitor_name);

        TaskOrResult::new(scoped_task::spawn(MonitoredDht::create(
            is_v4,
            socket,
            monitor,
            span,
            contacts_store,
        )))
    }

    async fn create(
        is_v4: bool,
        socket: quic::SideChannel,
        monitor: StateMonitor,
        span: Span,
        contacts_store: Option<Arc<dyn DhtContactsStoreTrait>>,
    ) -> Self {
        // TODO: load the DHT state from a previous save if it exists.
        let builder = MainlineDht::builder()
            .add_routers(DHT_ROUTERS.iter().copied())
            .set_read_only(false);

        // TODO: The reuse of initial contacts is incorrectly implemented, once the issue
        // https://github.com/equalitie/btdht/issues/8
        // is fixed, this can be uncommented again.
        //if let Some(contacts_store) = &contacts_store {
        //    let initial_contacts = Self::load_initial_contacts(is_v4, &**contacts_store).await;

        //    for contact in initial_contacts {
        //        builder = builder.add_node(contact);
        //    }
        //}

        let dht = builder
            .start(Socket(socket))
            // TODO: `start` only fails if the socket has been closed. That shouldn't be the case
            // there but better check.
            .unwrap();

        // Spawn a task to monitor the DHT status.
        let monitoring_task = {
            let dht = dht.clone();

            let first_bootstrap = monitor.make_value("first_bootstrap", "in progress");
            let probe_counter = monitor.make_value("probe_counter", 0);
            let is_running = monitor.make_value("is_running", false);
            let bootstrapped = monitor.make_value("bootstrapped", false);
            let good_nodes = monitor.make_value("good_nodes", 0);
            let questionable_nodes = monitor.make_value("questionable_nodes", 0);
            let buckets = monitor.make_value("buckets", 0);

            async move {
                tracing::info!("bootstrap started");

                if dht.bootstrapped(None).await {
                    *first_bootstrap.get() = "done";
                    tracing::info!("bootstrap complete");
                } else {
                    *first_bootstrap.get() = "failed";
                    tracing::error!("bootstrap failed");

                    // Don't `return`, instead halt here so that the `first_bootstrap` monitored value
                    // is preserved for the user to see.
                    pending::<()>().await;
                }

                loop {
                    *probe_counter.get() += 1;

                    if let Some(state) = dht.get_state().await {
                        *is_running.get() = true;
                        *bootstrapped.get() = true;
                        *good_nodes.get() = state.good_node_count;
                        *questionable_nodes.get() = state.questionable_node_count;
                        *buckets.get() = state.bucket_count;
                    } else {
                        *is_running.get() = false;
                        *bootstrapped.get() = false;
                        *good_nodes.get() = 0;
                        *questionable_nodes.get() = 0;
                        *buckets.get() = 0;
                    }

                    time::sleep(Duration::from_secs(5)).await;
                }
            }
        };
        let monitoring_task = monitoring_task.instrument(span.clone());
        let monitoring_task = scoped_task::spawn(monitoring_task);

        let _periodic_dht_node_load_task = contacts_store.map(|contacts_store| {
            scoped_task::spawn(
                Self::keep_reading_contacts(is_v4, dht.clone(), contacts_store).instrument(span),
            )
        });

        Self {
            dht,
            _monitoring_task: monitoring_task,
            _periodic_dht_node_load_task,
        }
    }

    /// Periodically read contacts from the `dht` and send it to `on_periodic_dht_node_load_tx`.
    async fn keep_reading_contacts(
        is_v4: bool,
        dht: MainlineDht,
        contacts_store: Arc<dyn DhtContactsStoreTrait>,
    ) {
        let mut reported_failure = false;

        // Give `MainlineDht` a chance to bootstrap.
        time::sleep(Duration::from_secs(10)).await;

        loop {
            let (good, questionable) = match dht.load_contacts().await {
                Ok((good, questionable)) => (good, questionable),
                Err(error) => {
                    tracing::warn!("DhtDiscovery stopped reading contacts: {error:?}");
                    break;
                }
            };

            // TODO: Make use of the information which is good and which questionable.
            let mix = good.union(&questionable);

            if is_v4 {
                let mix = mix.filter_map(|addr| match addr {
                    SocketAddr::V4(addr) => Some(*addr),
                    SocketAddr::V6(_) => None,
                });

                match contacts_store.store_v4(mix.collect()).await {
                    Ok(()) => reported_failure = false,
                    Err(error) => {
                        if !reported_failure {
                            reported_failure = true;
                            tracing::error!("DhtDiscovery failed to write contacts {error:?}");
                        }
                    }
                }
            } else {
                let mix = mix.filter_map(|addr| match addr {
                    SocketAddr::V4(_) => None,
                    SocketAddr::V6(addr) => Some(*addr),
                });

                match contacts_store.store_v6(mix.collect()).await {
                    Ok(()) => reported_failure = false,
                    Err(error) => {
                        if !reported_failure {
                            reported_failure = true;
                            tracing::error!("DhtDiscovery failed to write contacts {error:?}");
                        }
                    }
                }
            }

            time::sleep(Duration::from_secs(60)).await;
        }
    }

    // TODO: See comment where this function is used (it's also commented out).
    //async fn load_initial_contacts(
    //    is_v4: bool,
    //    contacts_store: &(impl DhtContactsStoreTrait + ?Sized),
    //) -> HashSet<SocketAddr> {
    //    if is_v4 {
    //        match contacts_store.load_v4().await {
    //            Ok(contacts) => contacts.iter().cloned().map(SocketAddr::V4).collect(),
    //            Err(error) => {
    //                tracing::error!("Failed to load DHT IPv4 contacts {:?}", error);
    //                Default::default()
    //            }
    //        }
    //    } else {
    //        match contacts_store.load_v6().await {
    //            Ok(contacts) => contacts.iter().cloned().map(SocketAddr::V6).collect(),
    //            Err(error) => {
    //                tracing::error!("Failed to load DHT IPv4 contacts {:?}", error);
    //                Default::default()
    //            }
    //        }
    //    }
    //}
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
    requests: Arc<BlockingMutex<HashMap<RequestId, mpsc::UnboundedSender<SeenPeer>>>>,
    wake_up_tx: watch::Sender<()>,
    task: Option<ScopedJoinHandle<()>>,
}

impl Lookup {
    fn start(
        dht_v4: Arc<Option<TaskOrResult<MonitoredDht>>>,
        dht_v6: Arc<Option<TaskOrResult<MonitoredDht>>>,
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

        Lookup {
            seen_peers,
            requests,
            wake_up_tx,
            task,
        }
    }

    // Start this same lookup on different DHT instances
    fn restart(
        &mut self,
        dht_v4: Arc<Option<TaskOrResult<MonitoredDht>>>,
        dht_v6: Arc<Option<TaskOrResult<MonitoredDht>>>,
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

    fn add_request(&mut self, id: RequestId, tx: mpsc::UnboundedSender<SeenPeer>) {
        for peer in self.seen_peers.collect() {
            tx.send(peer.clone()).unwrap_or(());
        }

        self.requests.lock().unwrap().insert(id, tx);
        // `unwrap_or` because if the network is down, there should be no tasks that listen to this
        // wake up request.
        self.wake_up_tx.send(()).unwrap_or(());
    }

    #[allow(clippy::too_many_arguments)]
    fn start_task(
        dht_v4: Arc<Option<TaskOrResult<MonitoredDht>>>,
        dht_v6: Arc<Option<TaskOrResult<MonitoredDht>>>,
        info_hash: InfoHash,
        seen_peers: Arc<SeenPeers>,
        requests: Arc<BlockingMutex<HashMap<RequestId, mpsc::UnboundedSender<SeenPeer>>>>,
        mut wake_up: watch::Receiver<()>,
        lookups_monitor: &StateMonitor,
        span: &Span,
    ) -> ScopedJoinHandle<()> {
        let monitor = lookups_monitor.make_child(format!("{info_hash:?}"));
        let state = monitor.make_value("state", "started");
        let next = monitor.make_value("next", SystemTime::now().into());

        let task = async move {
            let dht_v4 = match &*dht_v4 {
                Some(dht) => Some(dht.result().await),
                None => None,
            };

            let dht_v6 = match &*dht_v6 {
                Some(dht) => Some(dht.result().await),
                None => None,
            };

            // Wait for the first request to be created
            wake_up.changed().await.unwrap_or(());

            loop {
                seen_peers.start_new_round();

                tracing::debug!(?info_hash, "starting search");
                *state.get() = "making request";

                // find peers for the repo and also announce that we have it.
                let dhts = dht_v4.iter().chain(dht_v6.iter());

                let mut peers = Box::pin(stream::iter(dhts).flat_map(|dht| {
                    stream::once(async {
                        dht.dht.bootstrapped(Some(Duration::from_secs(10))).await;
                        dht.dht.search(info_hash, true)
                    })
                    .flatten()
                }));

                *state.get() = "awaiting results";

                while let Some(addr) = peers.next().await {
                    if let Some(peer) = seen_peers.insert(PeerAddr::Quic(addr)) {
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

struct Socket(quic::SideChannel);

#[async_trait]
impl btdht::SocketTrait for Socket {
    async fn send_to(&self, buf: &[u8], target: &SocketAddr) -> io::Result<()> {
        self.0.send_to(buf, target).await
    }

    async fn recv_from(&mut self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.0.recv_from(buf).await
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.local_addr()
    }
}

struct TaskOrResult<T> {
    task: AsyncMutex<Option<ScopedJoinHandle<T>>>,
    result: once_cell::sync::OnceCell<T>,
}

impl<T> TaskOrResult<T> {
    fn new(task: ScopedJoinHandle<T>) -> Self {
        Self {
            task: AsyncMutex::new(Some(task)),
            result: once_cell::sync::OnceCell::new(),
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
