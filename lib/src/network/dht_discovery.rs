use super::{
    peer_addr::PeerAddr,
    quic,
    seen_peers::{SeenPeer, SeenPeers},
};
use crate::scoped_task::{self, ScopedJoinHandle};
use async_trait::async_trait;
use btdht::{InfoHash, MainlineDht};
use chrono::{offset::Local, DateTime};
use futures_util::{stream, StreamExt};
use rand::Rng;
use std::{
    collections::HashMap,
    future::pending,
    io,
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
use tracing::{instrument::Instrument, Span};

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
    span: Span,
}

impl DhtDiscovery {
    pub fn new(
        socket_maker_v4: Option<quic::SideChannelMaker>,
        socket_maker_v6: Option<quic::SideChannelMaker>,
    ) -> Self {
        let v4 = Mutex::new(RestartableDht::new(socket_maker_v4));
        let v6 = Mutex::new(RestartableDht::new(socket_maker_v6));
        let lookups = Arc::new(Mutex::new(HashMap::default()));
        let span = tracing::info_span!("dht");

        Self {
            v4,
            v6,
            lookups,
            next_id: AtomicU64::new(0),
            span,
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

        let dht_v4 = v4.fetch(&self.span);
        let dht_v6 = v6.fetch(&self.span);

        for (info_hash, lookup) in &mut *lookups {
            lookup.restart(dht_v4.clone(), dht_v6.clone(), *info_hash, &self.span);
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
                let dht_v4 = self.v4.lock().unwrap().fetch(&self.span);
                let dht_v6 = self.v6.lock().unwrap().fetch(&self.span);

                Lookup::start(dht_v4, dht_v6, info_hash, &self.span)
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
    fn fetch(&mut self, span: &Span) -> Arc<Option<MonitoredDht>> {
        if let Some(dht) = self.dht.upgrade() {
            dht
        } else if let Some(maker) = &self.socket_maker {
            let socket = maker.make();
            let dht = MonitoredDht::start(socket, span);
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
}

impl MonitoredDht {
    fn start(socket: quic::SideChannel, span: &Span) -> Self {
        // TODO: Unwrap
        let local_addr = socket.local_addr().unwrap();

        let span = match local_addr {
            SocketAddr::V4(_) => tracing::info_span!(parent: span, "IPv4"),
            SocketAddr::V6(_) => tracing::info_span!(parent: span, "IPv6"),
        };

        // TODO: load the DHT state from a previous save if it exists.
        let dht = MainlineDht::builder()
            .add_routers(DHT_ROUTERS.iter().copied())
            .set_read_only(false)
            .start(socket)
            // Unwrap OK because `start` only fails if it can't get `local_addr` out of the socket, but
            // since we just succeeded in binding the socket above, that shouldn't happen.
            .unwrap();

        // Spawn a task to monitor the DHT status.
        let monitoring_task = {
            let dht = dht.clone();

            async move {
                tracing::trace!(first_bootstrap = "in progress");

                if dht.bootstrapped(None).await {
                    tracing::trace!(first_bootstrap = "done");
                    tracing::info!("bootstrap complete");
                } else {
                    tracing::trace!(first_bootstrap = "failed");
                    tracing::error!("bootstrap failed");

                    // Don't `return`, instead halt here so that the `first_bootstrap` monitored value
                    // is preserved for the user to see.
                    pending::<()>().await;
                }

                let mut probe_counter = 0;

                loop {
                    probe_counter += 1;

                    if let Some(state) = dht.get_state().await {
                        tracing::trace!(
                            probe_counter,
                            is_running = state.is_running,
                            bootstrapped = state.bootstrapped,
                            good_node_count = state.good_node_count,
                            questionable_node_count = state.questionable_node_count,
                            bucket_count = state.bucket_count,
                        );
                    } else {
                        tracing::trace!(
                            probe_counter,
                            is_running = false,
                            bootstrapped = false,
                            good_node_count = 0,
                            questionable_node_count = 0,
                            bucket_count = 0,
                        );
                    }

                    time::sleep(Duration::from_secs(5)).await;
                }
            }
        };
        let monitoring_task = monitoring_task.instrument(span);
        let monitoring_task = scoped_task::spawn(monitoring_task);

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
    task: Option<ScopedJoinHandle<()>>,
}

impl Lookup {
    fn start(
        dht_v4: Arc<Option<MonitoredDht>>,
        dht_v6: Arc<Option<MonitoredDht>>,
        info_hash: InfoHash,
        span: &Span,
    ) -> Self {
        let (wake_up_tx, mut wake_up_rx) = watch::channel(());
        // Mark the initial value as seen so the change notification is not triggered immediately
        // but only when we create the first request.
        wake_up_rx.borrow_and_update();

        let seen_peers = Arc::new(SeenPeers::new());
        let requests = Arc::new(Mutex::new(HashMap::new()));

        let task = if dht_v4.is_some() || dht_v6.is_some() {
            let span = tracing::info_span!(parent: span, "lookup", ?info_hash);

            Some(Self::start_task(
                dht_v4,
                dht_v6,
                info_hash,
                seen_peers.clone(),
                requests.clone(),
                wake_up_rx,
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
        dht_v4: Arc<Option<MonitoredDht>>,
        dht_v6: Arc<Option<MonitoredDht>>,
        info_hash: InfoHash,
        span: &Span,
    ) {
        if dht_v4.is_none() && dht_v6.is_none() {
            self.task.take();
            return;
        }

        let span = tracing::info_span!(parent: span, "lookup", ?info_hash);
        let task = Self::start_task(
            dht_v4,
            dht_v6,
            info_hash,
            self.seen_peers.clone(),
            self.requests.clone(),
            self.wake_up_tx.subscribe(),
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

    fn start_task(
        dht_v4: Arc<Option<MonitoredDht>>,
        dht_v6: Arc<Option<MonitoredDht>>,
        info_hash: InfoHash,
        seen_peers: Arc<SeenPeers>,
        requests: Arc<Mutex<HashMap<RequestId, mpsc::UnboundedSender<SeenPeer>>>>,
        mut wake_up: watch::Receiver<()>,
        span: Span,
    ) -> ScopedJoinHandle<()> {
        let task = async move {
            tracing::trace!(state = "started");

            // Wait for the first request to be created
            wake_up.changed().await.unwrap_or(());

            loop {
                seen_peers.start_new_round();

                tracing::debug!("starting search");
                tracing::trace!(state = "making request");

                // find peers for the repo and also announce that we have it.
                let dhts = dht_v4.iter().chain(dht_v6.iter());

                let mut peers = Box::pin(stream::iter(dhts).flat_map(|dht| {
                    stream::once(async {
                        dht.dht.bootstrapped(Some(Duration::from_secs(10))).await;
                        dht.dht.search(info_hash, true)
                    })
                    .flatten()
                }));

                tracing::trace!(state = "awaiting results");

                while let Some(addr) = peers.next().await {
                    if let Some(peer) = seen_peers.insert(PeerAddr::Quic(addr)) {
                        tracing::debug!("found peer {:?}", peer.initial_addr());

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
                    tracing::trace!(state = format!("sleeping until {}", time.format("%T")));
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

#[async_trait]
impl btdht::SocketTrait for quic::SideChannel {
    async fn send_to(&self, buf: &[u8], target: &SocketAddr) -> io::Result<()> {
        self.send_to(buf, target).await
    }

    async fn recv_from(&mut self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.recv_from(buf).await
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.local_addr()
    }
}
