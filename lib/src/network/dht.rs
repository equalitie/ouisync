mod lookup_stream;
mod monitored;
mod restartable;

pub use lookup_stream::DhtLookupStream;
use slab::Slab;

use crate::{network::dht::restartable::ObservableDht, sync::WatchSenderExt};

use super::{
    peer_addr::PeerAddr,
    seen_peers::{SeenPeer, SeenPeers},
};
use async_trait::async_trait;
use btdht::{InfoHash, MainlineDht};
use chrono::{DateTime, offset::Local};
use deadlock::BlockingMutex;
use futures_util::{Stream, StreamExt, future, stream};
use net::quic;
use rand::Rng;
use restartable::RestartableDht;
use state_monitor::StateMonitor;
use std::{
    collections::{HashMap, HashSet},
    io,
    net::{SocketAddr, SocketAddrV4, SocketAddrV6},
    pin::pin,
    sync::{Arc, Weak},
    time::{Duration, SystemTime},
};
use tokio::{
    select,
    sync::{mpsc, watch},
    task, time,
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

// Max time to wait for the DHT to bootstrap before starting a lookup on it.
const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(10);

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
    v4: RestartableDht,
    v6: RestartableDht,
    lookups: Arc<BlockingMutex<LookupMap>>,
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

        let v4 = RestartableDht::new(contacts.clone(), monitor.clone());
        v4.bind(socket_maker_v4);
        v4.set_routers(routers.clone());

        let v6 = RestartableDht::new(contacts, monitor.clone());
        v6.bind(socket_maker_v6);
        v6.set_routers(routers);

        let lookups = Arc::new(BlockingMutex::new(HashMap::default()));

        let lookups_monitor = monitor.make_child("lookups");

        Self {
            v4,
            v6,
            lookups,
            lookups_monitor,
            span: Span::current(),
        }
    }

    /// Binds new sockets to the DHT instances, rebootstraps the DHTs and restarts any ongoing
    /// lookups.
    pub fn rebind(
        &self,
        socket_maker_v4: Option<quic::SideChannelMaker>,
        socket_maker_v6: Option<quic::SideChannelMaker>,
    ) {
        self.v4.bind(socket_maker_v4);
        self.v6.bind(socket_maker_v6);
    }

    /// Changes the DHT routers, rebootstraps the DHTs and restart any ongoing lookups.
    pub fn set_routers(&self, routers: HashSet<String>) {
        self.v4.set_routers(routers.clone());
        self.v6.set_routers(routers);
    }

    /// Returns the current DHT routers.
    pub fn routers(&self) -> HashSet<String> {
        self.v4
            .routers()
            .into_iter()
            .chain(self.v6.routers())
            .collect()
    }

    /// Starts a DHT lookup/announce for the given infohash. Sends the found peer addresses into the
    /// given sender.
    pub fn start_lookup(
        &self,
        info_hash: InfoHash,
        announce: bool,
        event_tx: mpsc::UnboundedSender<DhtEvent>,
    ) -> LookupRequest {
        let key = self
            .lookups
            .lock()
            .unwrap()
            .entry(info_hash)
            .or_insert_with(|| {
                let v4 = self.v4.observe();
                let v6 = self.v6.observe();
                Lookup::start(v4, v6, info_hash, &self.lookups_monitor, self.span.clone())
            })
            .add_request(announce, event_tx);

        LookupRequest {
            lookups: Arc::downgrade(&self.lookups),
            info_hash,
            key,
        }
    }

    /// Creates a "pin" which keeps the DHT instances running once they have been started. This
    /// prevents the DHTs to shut down even when there are no more ongoing lookups. This is useful
    /// if one wants to avoid having to rebootstrap the DHT when doing another lookup in the future.
    ///
    /// Note pinning the DHT doesn't bootstrap it by itself - it happens lazyly on the first lookup.
    pub fn pin(&self) -> DhtPin {
        DhtPin {
            _v4: self.v4.observe(),
            _v6: self.v6.observe(),
        }
    }
}

pub struct LookupRequest {
    lookups: Weak<BlockingMutex<LookupMap>>,
    info_hash: InfoHash,
    key: usize,
}

impl Drop for LookupRequest {
    fn drop(&mut self) {
        if let Some(lookups) = self.lookups.upgrade() {
            let mut lookups = lookups.lock().unwrap();

            let empty = if let Some(lookup) = lookups.get_mut(&self.info_hash) {
                lookup.requests_tx.send_modify_return(|requests| {
                    requests.remove(self.key);
                    requests.is_empty()
                })
            } else {
                false
            };

            if empty {
                lookups.remove(&self.info_hash);
            }
        }
    }
}

struct RequestData {
    event_tx: mpsc::UnboundedSender<DhtEvent>,
    announce: bool,
}

struct Lookup {
    requests_tx: watch::Sender<Slab<RequestData>>,
}

impl Lookup {
    fn start(
        v4: ObservableDht,
        v6: ObservableDht,
        info_hash: InfoHash,
        monitor: &StateMonitor,
        span: Span,
    ) -> Self {
        let (requests_tx, requests_rx) = watch::channel(Slab::new());
        let monitor = monitor.make_child(format!("{info_hash:?}"));

        task::spawn(Self::run(v4, v6, info_hash, requests_rx, monitor).instrument(span));

        Self { requests_tx }
    }

    fn add_request(&mut self, announce: bool, event_tx: mpsc::UnboundedSender<DhtEvent>) -> usize {
        self.requests_tx
            .send_modify_return(|requests| requests.insert(RequestData { event_tx, announce }))
    }

    #[allow(clippy::too_many_arguments)]
    async fn run(
        dht_v4: ObservableDht,
        dht_v6: ObservableDht,
        info_hash: InfoHash,
        mut requests_rx: watch::Receiver<Slab<RequestData>>,
        monitor: StateMonitor,
    ) {
        let seen_peers = SeenPeers::new();

        let state = monitor.make_value("state", "started");
        let next = monitor.make_value("next", SystemTime::now().into());

        loop {
            // Wait until there is at least one request
            if requests_rx
                .wait_for(|requests| !requests.is_empty())
                .await
                .is_err()
            {
                return;
            }

            // Wait until at least one DHT has been enabled and all enabled DHTs have been started.
            let dhts = async {
                loop {
                    select! {
                        _ = dht_v4.enabled() => (),
                        _ = dht_v6.enabled() => (),
                    }

                    let (v4, v6) =
                        future::join(dht_v4.started_or_disabled(), dht_v6.started_or_disabled())
                            .await;

                    if v4.is_some() || v6.is_some() {
                        break (v4, v6);
                    }
                }
            };

            let (dht_v4, dht_v6) = select! {
                dhts = dhts => dhts,
                _ = closed(&mut requests_rx) => return,
            };

            seen_peers.start_new_round();

            tracing::debug!(
                ?info_hash,
                v4 = dht_v4.is_some(),
                v6 = dht_v6.is_some(),
                "starting search"
            );

            // If at least one request wants to announce, we do announce. Otherwise we do only
            // lookup.
            let announce = requests_rx
                .borrow()
                .iter()
                .any(|(_, request)| request.announce);

            let peers_v4 = search(dht_v4.as_ref(), info_hash, announce);
            let peers_v6 = search(dht_v6.as_ref(), info_hash, announce);
            let mut peers = pin!(stream::select(peers_v4, peers_v6));

            *state.get() = "awaiting results";

            loop {
                let addr = select! {
                    addr = peers.next() => {
                        if let Some(addr) = addr {
                            addr
                        } else {
                            break;
                        }
                    }
                    _ = closed(&mut requests_rx) => return,
                };

                if let Some(peer) = seen_peers.insert(PeerAddr::Quic(addr)) {
                    for (_, request) in requests_rx.borrow().iter() {
                        request
                            .event_tx
                            .send(DhtEvent::PeerFound(peer.clone()))
                            .ok();
                    }
                }
            }

            for (_, request) in requests_rx.borrow().iter() {
                request.event_tx.send(DhtEvent::RoundEnded).ok();
            }

            // sleep a random duration before the next search
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
                _ = closed(&mut requests_rx) => return,
            }
        }
    }
}

type LookupMap = HashMap<InfoHash, Lookup>;

/// Ensures the DHT instances, once started, remain running until the pin is in scope. See
/// [DhtDiscovery::pin] for more info.
pub struct DhtPin {
    _v4: ObservableDht,
    _v6: ObservableDht,
}

/// Returns when the channel gets closed (all senders get closed).
async fn closed<T>(rx: &mut watch::Receiver<T>) {
    while rx.changed().await.is_ok() {}
}

/// Waits until the dht bootstraps then perform a lookup or announce on it for the given infohash.
/// If `dht` is `None`, returns empty stream.
fn search<'a>(
    dht: Option<&'a MainlineDht>,
    info_hash: InfoHash,
    announce: bool,
) -> impl Stream<Item = SocketAddr> + 'a {
    stream::iter(dht)
        .then(move |dht| async move {
            time::timeout(BOOTSTRAP_TIMEOUT, dht.bootstrapped())
                .await
                .ok();

            dht.search(info_hash, announce)
        })
        .flatten()
}
