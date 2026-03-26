use super::{
    peer_addr::PeerAddr,
    seen_peers::{SeenPeer, SeenPeers},
};
use async_trait::async_trait;
use btdht::{InfoHash, MainlineDht};
use chrono::{DateTime, offset::Local};
use deadlock::{AsyncMutex, BlockingMutex};
use futures_util::{StreamExt, stream};
use net::{quic, udp::DatagramSocket};
use rand::Rng;
use scoped_task::ScopedJoinHandle;
use state_monitor::StateMonitor;
use std::{
    collections::{HashMap, HashSet},
    future::pending,
    io,
    net::{SocketAddr, SocketAddrV4, SocketAddrV6},
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

#[derive(Clone)]
pub struct DhtOptions {
    /// DHT bootstrap nodes
    pub routers: HashSet<String>,
    /// Additional DHT contacts
    pub contacts: Option<Arc<dyn DhtContactsStoreTrait>>,
}

impl Default for DhtOptions {
    fn default() -> Self {
        Self {
            routers: DEFAULT_DHT_ROUTERS
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
            contacts: None,
        }
    }
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
        options: DhtOptions,
        monitor: StateMonitor,
    ) -> Self {
        let v4 = BlockingMutex::new(RestartableDht::new(socket_maker_v4, options.clone()));
        let v6 = BlockingMutex::new(RestartableDht::new(socket_maker_v6, options));

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
}

// Wrapper for a DHT instance that can be stopped and restarted at any point.
struct RestartableDht {
    socket_maker: Option<quic::SideChannelMaker>,
    dht: Weak<Option<TaskOrResult<MonitoredDht>>>,
    options: DhtOptions,
}

impl RestartableDht {
    fn new(socket_maker: Option<quic::SideChannelMaker>, options: DhtOptions) -> Self {
        Self {
            socket_maker,
            dht: Weak::new(),
            options,
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
            let dht = MonitoredDht::start(socket, self.options.clone(), monitor, span);
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
        options: DhtOptions,
        parent_monitor: &StateMonitor,
        span: &Span,
    ) -> TaskOrResult<Self> {
        // TODO: Unwrap
        let local_addr = socket.local_addr().unwrap();

        let (is_v4, monitor_name, span) = match local_addr {
            SocketAddr::V4(_) => (true, "IPv4", tracing::info_span!(parent: span, "DHT/IPv4")),
            SocketAddr::V6(_) => (false, "IPv6", tracing::info_span!(parent: span, "DHT/IPv6")),
        };

        let monitor = parent_monitor.make_child(monitor_name);

        TaskOrResult::new(scoped_task::spawn(MonitoredDht::create(
            is_v4, socket, options, monitor, span,
        )))
    }

    async fn create(
        is_v4: bool,
        socket: quic::SideChannel,
        options: DhtOptions,
        monitor: StateMonitor,
        span: Span,
    ) -> Self {
        // TODO: load the DHT state from a previous save if it exists.
        let mut builder = MainlineDht::builder()
            .add_routers(options.routers)
            .set_read_only(false);

        if let Some(contacts_store) = &options.contacts {
            let initial_contacts = Self::load_initial_contacts(is_v4, &**contacts_store).await;

            for contact in initial_contacts {
                builder = builder.add_node(contact);
            }
        }

        let dht = span
            .in_scope(|| builder.start(Socket(socket)))
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

                if dht.bootstrapped().await {
                    *first_bootstrap.get() = "done";
                    tracing::info!("bootstrap complete");
                } else {
                    *first_bootstrap.get() = "failed";
                    tracing::error!("bootstrap failed");

                    // Don't `return`, instead halt here so that the `first_bootstrap` monitored
                    // value is preserved for the user to see.
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

        let _periodic_dht_node_load_task = options.contacts.map(|contacts_store| {
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

    async fn load_initial_contacts(
        is_v4: bool,
        contacts_store: &(impl DhtContactsStoreTrait + ?Sized),
    ) -> HashSet<SocketAddr> {
        if is_v4 {
            match contacts_store.load_v4().await {
                Ok(contacts) => contacts.iter().cloned().map(SocketAddr::V4).collect(),
                Err(error) => {
                    tracing::error!("Failed to load DHT IPv4 contacts {:?}", error);
                    Default::default()
                }
            }
        } else {
            match contacts_store.load_v6().await {
                Ok(contacts) => contacts.iter().cloned().map(SocketAddr::V6).collect(),
                Err(error) => {
                    tracing::error!("Failed to load DHT IPv4 contacts {:?}", error);
                    Default::default()
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
        dht_v4: Arc<Option<TaskOrResult<MonitoredDht>>>,
        dht_v6: Arc<Option<TaskOrResult<MonitoredDht>>>,
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

struct Socket(quic::SideChannel);

#[async_trait]
impl btdht::SocketTrait for Socket {
    async fn send_to(&self, buf: &[u8], target: &SocketAddr) -> io::Result<()> {
        self.0.send_to(buf, *target).await?;
        Ok(())
    }

    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.0.recv_from(buf).await
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.local_addr()
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
