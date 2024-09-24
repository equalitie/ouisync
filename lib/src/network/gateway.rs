use super::{ip, peer_addr::PeerAddr, peer_source::PeerSource, raw, seen_peers::SeenPeer};
use crate::sync::atomic_slot::AtomicSlot;
use backoff::{backoff::Backoff, ExponentialBackoffBuilder};
use net::{
    quic,
    tcp::{TcpListener, TcpStream},
};
use scoped_task::ScopedJoinHandle;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
};
use thiserror::Error;
use tokio::{
    select,
    sync::{mpsc, oneshot, watch},
    time::{self, Duration},
};
use tracing::{field, Instrument, Span};

/// Established incoming and outgoing connections.
pub(super) struct Gateway {
    stacks: AtomicSlot<Stacks>,
    incoming_tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
    connectivity_tx: watch::Sender<Connectivity>,
}

impl Gateway {
    /// Create a new `Gateway` that is initially disabled.
    ///
    /// `incoming_tx` is the sender for the incoming connections.
    pub fn new(incoming_tx: mpsc::Sender<(raw::Stream, PeerAddr)>) -> Self {
        let stacks = Stacks::unbound();
        let stacks = AtomicSlot::new(stacks);

        Self {
            stacks,
            incoming_tx,
            connectivity_tx: watch::channel(Connectivity::Disabled).0,
        }
    }

    pub fn listener_local_addrs(&self) -> Vec<PeerAddr> {
        let stacks = self.stacks.read();
        [
            stacks
                .quic_listener_local_addr_v4()
                .copied()
                .map(PeerAddr::Quic),
            stacks
                .quic_listener_local_addr_v6()
                .copied()
                .map(PeerAddr::Quic),
            stacks
                .tcp_listener_local_addr_v4()
                .copied()
                .map(PeerAddr::Tcp),
            stacks
                .tcp_listener_local_addr_v6()
                .copied()
                .map(PeerAddr::Tcp),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// Binds the gateway to the specified addresses. Rebinds if already bound.
    pub async fn bind(
        &self,
        bind: &StackAddresses,
    ) -> (
        Option<quic::SideChannelMaker>,
        Option<quic::SideChannelMaker>,
    ) {
        let (next, side_channel_maker_v4, side_channel_maker_v6) =
            Stacks::bind(bind, self.incoming_tx.clone()).await;

        let prev = self.stacks.swap(next);
        let next = self.stacks.read();

        if prev.quic_v4.is_some() && next.quic_v4.is_none() {
            tracing::info!("Terminated IPv4 QUIC stack");
        }

        if prev.quic_v6.is_some() && next.quic_v6.is_none() {
            tracing::info!("Terminated IPv6 QUIC stack");
        }

        if prev.tcp_v4.is_some() && next.tcp_v4.is_none() {
            tracing::info!("Terminated IPv4 TCP stack");
        }

        if prev.tcp_v6.is_some() && next.tcp_v6.is_none() {
            tracing::info!("Terminated IPv6 TCP stack");
        }

        self.connectivity_tx.send_if_modified(|conn| {
            let old_conn = *conn;
            *conn = Connectivity::infer(&next.addresses().ip_addrs());
            *conn != old_conn
        });

        prev.close();

        (side_channel_maker_v4, side_channel_maker_v6)
    }

    pub fn connectivity(&self) -> Connectivity {
        *self.connectivity_tx.borrow()
    }

    pub fn connectivity_subscribe(&self) -> watch::Receiver<Connectivity> {
        self.connectivity_tx.subscribe()
    }

    pub async fn connect_with_retries(
        &self,
        peer: &SeenPeer,
        source: PeerSource,
    ) -> Option<raw::Stream> {
        if !ok_to_connect(peer.addr_if_seen()?.socket_addr(), source) {
            tracing::debug!("Invalid peer address - discarding");
            return None;
        }

        let create_backoff = || {
            ExponentialBackoffBuilder::new()
                .with_initial_interval(Duration::from_millis(200))
                .with_max_interval(Duration::from_secs(10))
                // We'll continue trying for as long as `peer.addr().is_some()`.
                .with_max_elapsed_time(None)
                .build()
        };

        let mut backoff = create_backoff();

        let mut hole_punching_task = None;
        let mut last_conn = Connectivity::Disabled;
        let mut connectivity_rx = self.connectivity_subscribe();

        loop {
            // Note: This needs to be probed each time the loop starts. When the `addr` fn returns
            // `None` that means whatever discovery mechanism (LocalDiscovery or DhtDiscovery)
            // found it is no longer seeing it.
            let addr = *peer.addr_if_seen()?;

            // Wait for `Connectivity` to be such that it allows us to connect. E.g. if it's
            // `Connectivity::LocalOnly` then we can't connect to global addresses.
            loop {
                let conn = *connectivity_rx.borrow_and_update();

                if conn.allows_connection_to(addr) {
                    if conn.is_less_restrictive_than(last_conn) {
                        backoff = create_backoff();
                    }
                    last_conn = conn;
                    break;
                }

                // Unwrap is OK because `connectivity_tx` lives in `self` so it can't be destroyed
                // while this function is being executed.
                connectivity_rx.changed().await.unwrap();
            }

            // Note: we need to grab fresh stacks on each loop because the network might get
            // re-bound in the meantime which would change the connectors.
            let stacks = self.stacks.read();

            if hole_punching_task.is_none() {
                hole_punching_task = stacks.start_punching_holes(addr);
            }

            match stacks.connect(addr).await {
                Ok(socket) => {
                    // This condition is to avoid a race condition for when `Connectivity` grants
                    // us connection, but then there is a re-`bind` after which the connection
                    // should no longer exist. From now on it should be ok if re-`bind` happens
                    // because it'll also call `network.rs/Inner/disconnect_all` and this socket
                    // will disconnect.  TODO: This is a bit dirty because it depends on a
                    // behaviour of a class which this module doesn't controll. Consider somehow
                    // disconnecting the socket explicity when it's creation `stack` is destroyed
                    // (note this already works for QUIC connections because they share a common
                    // UDP socket inside `stack`, but it does not work this way for TCP
                    // connections).
                    if !connectivity_rx.borrow().allows_connection_to(addr) {
                        continue;
                    }

                    return Some(socket);
                }
                Err(error) => {
                    tracing::debug!(?error, "Connection failed");

                    if error.is_localy_closed() {
                        // Connector locally closed - no point in retrying.
                        return None;
                    }

                    match backoff.next_backoff() {
                        Some(duration) => {
                            tracing::debug!("Next connection attempt in {:?}", duration);
                            time::sleep(duration).await;
                        }
                        // We set max elapsed time to None above.
                        None => unreachable!(),
                    }
                }
            }
        }
    }

    pub fn addresses(&self) -> StackAddresses {
        self.stacks.read().addresses()
    }
}

#[derive(Debug, Error)]
pub(super) enum ConnectError {
    #[error("TCP error")]
    Tcp(std::io::Error),
    #[error("QUIC error")]
    Quic(quic::Error),
    #[error("No corresponding QUIC connector")]
    NoSuitableQuicConnector,
}

impl ConnectError {
    fn is_localy_closed(&self) -> bool {
        matches!(
            self,
            Self::Quic(quic::Error::Connection(
                quic::ConnectionError::LocallyClosed
            ))
        )
    }
}

struct Stacks {
    quic_v4: Option<QuicStack>,
    quic_v6: Option<QuicStack>,
    tcp_v4: Option<TcpStack>,
    tcp_v6: Option<TcpStack>,
}

impl Stacks {
    fn unbound() -> Self {
        Self {
            quic_v4: None,
            quic_v6: None,
            tcp_v4: None,
            tcp_v6: None,
        }
    }

    async fn bind(
        bind: &StackAddresses,
        incoming_tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
    ) -> (
        Self,
        Option<quic::SideChannelMaker>,
        Option<quic::SideChannelMaker>,
    ) {
        let (quic_v4, side_channel_maker_v4) = if let Some(addr) = bind.quic_v4 {
            QuicStack::new(addr, incoming_tx.clone())
                .await
                .map(|(stack, side_channel)| (Some(stack), Some(side_channel)))
                .unwrap_or((None, None))
        } else {
            (None, None)
        };

        let (quic_v6, side_channel_maker_v6) = if let Some(addr) = bind.quic_v6 {
            QuicStack::new(addr, incoming_tx.clone())
                .await
                .map(|(stack, side_channel)| (Some(stack), Some(side_channel)))
                .unwrap_or((None, None))
        } else {
            (None, None)
        };

        let tcp_v4 = if let Some(addr) = bind.tcp_v4 {
            TcpStack::new(addr, incoming_tx.clone()).await
        } else {
            None
        };

        let tcp_v6 = if let Some(addr) = bind.tcp_v6 {
            TcpStack::new(addr, incoming_tx).await
        } else {
            None
        };

        let this = Self {
            quic_v4,
            quic_v6,
            tcp_v4,
            tcp_v6,
        };

        (this, side_channel_maker_v4, side_channel_maker_v6)
    }

    fn addresses(&self) -> StackAddresses {
        StackAddresses {
            quic_v4: self.quic_v4.as_ref().map(|stack| stack.listener_local_addr),
            quic_v6: self.quic_v6.as_ref().map(|stack| stack.listener_local_addr),
            tcp_v4: self.tcp_v4.as_ref().map(|stack| stack.listener_local_addr),
            tcp_v6: self.tcp_v6.as_ref().map(|stack| stack.listener_local_addr),
        }
    }

    fn quic_listener_local_addr_v4(&self) -> Option<&SocketAddr> {
        self.quic_v4
            .as_ref()
            .map(|stack| &stack.listener_local_addr)
    }

    fn quic_listener_local_addr_v6(&self) -> Option<&SocketAddr> {
        self.quic_v6
            .as_ref()
            .map(|stack| &stack.listener_local_addr)
    }

    fn tcp_listener_local_addr_v4(&self) -> Option<&SocketAddr> {
        self.tcp_v4.as_ref().map(|stack| &stack.listener_local_addr)
    }

    fn tcp_listener_local_addr_v6(&self) -> Option<&SocketAddr> {
        self.tcp_v6.as_ref().map(|stack| &stack.listener_local_addr)
    }

    async fn connect(&self, addr: PeerAddr) -> Result<raw::Stream, ConnectError> {
        match addr {
            PeerAddr::Tcp(addr) => TcpStream::connect(addr)
                .await
                .map(raw::Stream::Tcp)
                .map_err(ConnectError::Tcp),
            PeerAddr::Quic(addr) => {
                let stack = self
                    .quic_stack_for(&addr.ip())
                    .ok_or(ConnectError::NoSuitableQuicConnector)?;

                stack
                    .connector
                    .connect(addr)
                    .await
                    .map(raw::Stream::Quic)
                    .map_err(ConnectError::Quic)
            }
        }
    }

    fn start_punching_holes(&self, addr: PeerAddr) -> Option<scoped_task::ScopedJoinHandle<()>> {
        if !addr.is_quic() {
            return None;
        }

        if !ip::is_global(&addr.ip()) {
            return None;
        }

        let stack = self.quic_stack_for(&addr.ip())?;

        let sender = stack.hole_puncher.clone();
        let task = scoped_task::spawn(
            async move {
                use rand::Rng;

                tracing::trace!("Hole punching started");

                // Using RAII to log the message even when the task is aborted.
                struct Guard(Span);

                impl Drop for Guard {
                    fn drop(&mut self) {
                        tracing::trace!(parent: &self.0, "Hole punching stopped");
                    }
                }

                let _guard = Guard(Span::current());

                let addr = addr.socket_addr();
                loop {
                    let duration = rand::thread_rng().gen_range(5_000..15_000);
                    let duration = Duration::from_millis(duration);

                    // Sleep first because the `connect` function that is normally called right
                    // after this function will send a SYN packet right a way, so no need to do
                    // double work here.
                    time::sleep(duration).await;
                    // TODO: Consider using something non-identifiable (random) but something that
                    // won't interfere with (will be ignored by) the quic and btdht protocols.
                    let msg = b"punch";
                    match sender.send_to(msg, addr).await {
                        Ok(()) => (),
                        Err(error) => tracing::warn!("Hole punch failed: {:?}", error),
                    }
                }
            }
            .instrument(Span::current()),
        );

        Some(task)
    }

    fn quic_stack_for(&self, ip: &IpAddr) -> Option<&QuicStack> {
        match ip {
            IpAddr::V4(_) => self.quic_v4.as_ref(),
            IpAddr::V6(_) => self.quic_v6.as_ref(),
        }
    }

    fn close(&self) {
        if let Some(stack) = &self.quic_v4 {
            stack.close();
        }

        if let Some(stack) = &self.quic_v6 {
            stack.close();
        }
    }
}

struct QuicStack {
    listener_local_addr: SocketAddr,
    listener_task: ScopedJoinHandle<()>,
    connector: quic::Connector,
    hole_puncher: quic::SideChannelSender,
}

impl QuicStack {
    async fn new(
        bind_addr: SocketAddr,
        incoming_tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
    ) -> Option<(Self, quic::SideChannelMaker)> {
        let span = tracing::info_span!("listener", addr = field::Empty);

        let (connector, listener, side_channel_maker) = match quic::configure(bind_addr).await {
            Ok((connector, listener, side_channel_maker)) => {
                span.record(
                    "addr",
                    field::display(PeerAddr::Quic(*listener.local_addr())),
                );
                tracing::info!(parent: &span, "Listener started");

                (connector, listener, side_channel_maker)
            }
            Err(error) => {
                tracing::warn!(
                    parent: &span,
                    bind_addr = %PeerAddr::Quic(bind_addr),
                    ?error,
                    "Failed to start listener"
                );
                return None;
            }
        };

        let listener_local_addr = *listener.local_addr();
        let listener_task =
            scoped_task::spawn(run_quic_listener(listener, incoming_tx).instrument(span));

        let hole_puncher = side_channel_maker.make().sender();

        let this = Self {
            connector,
            listener_local_addr,
            listener_task,
            hole_puncher,
        };

        Some((this, side_channel_maker))
    }

    fn close(&self) {
        self.listener_task.abort();
        self.connector.close();
    }
}

struct TcpStack {
    listener_local_addr: SocketAddr,
    _listener_task: ScopedJoinHandle<()>,
}

impl TcpStack {
    async fn new(
        bind_addr: SocketAddr,
        incoming_tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
    ) -> Option<Self> {
        let span = tracing::info_span!("listener", addr = field::Empty);

        let listener = match TcpListener::bind(bind_addr).await {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(
                    parent: &span,
                    bind_addr = %PeerAddr::Tcp(bind_addr),
                    ?error,
                    "Failed to start listener",
                );
                return None;
            }
        };

        let listener_local_addr = match listener.local_addr() {
            Ok(addr) => {
                span.record("addr", field::display(PeerAddr::Tcp(addr)));
                tracing::info!(parent: &span, "Listener started");

                addr
            }
            Err(error) => {
                tracing::warn!(
                    parent: &span,
                    bind_addr = %PeerAddr::Tcp(bind_addr),
                    ?error,
                    "Failed to get listener local address",
                );
                return None;
            }
        };

        let listener_task =
            scoped_task::spawn(run_tcp_listener(listener, incoming_tx).instrument(span));

        Some(Self {
            listener_local_addr,
            _listener_task: listener_task,
        })
    }
}

async fn run_tcp_listener(listener: TcpListener, tx: mpsc::Sender<(raw::Stream, PeerAddr)>) {
    loop {
        let result = select! {
            result = listener.accept() => result,
            _ = tx.closed() => break,
        };

        match result {
            Ok((stream, addr)) => {
                tx.send((raw::Stream::Tcp(stream), PeerAddr::Tcp(addr)))
                    .await
                    .ok();
            }
            Err(error) => {
                tracing::error!(?error, "Failed to accept connection");
                break;
            }
        }
    }
}

async fn run_quic_listener(
    mut listener: quic::Acceptor,
    tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
) {
    // Using `futures_util::stream::FuturesUnordered` may have been a nicer solution but I'm not
    // sure whether `quic::Acceptor::accept()` is cancel safe.
    let connectings = Arc::new(Mutex::new(HashMap::new()));
    let mut next_connecting_id = 0;

    loop {
        let result = select! {
            result = listener.accept() => result,
            _ = tx.closed() => break,
        };

        match result {
            Some(connecting) => {
                // Using this channel to ensure the task is not removed from `connectings` before
                // it's inserted.
                let (start_task_tx, start_task_rx) = oneshot::channel();

                let connecting_id = next_connecting_id;
                next_connecting_id += 1;

                // Spawn so we can start listening for the next connection ASAP.
                let task = scoped_task::spawn({
                    let tx = tx.clone();
                    let connectings = connectings.clone();
                    async move {
                        if start_task_rx.await.is_ok() {
                            match connecting.finish().await {
                                Ok(socket) => {
                                    let addr = *socket.remote_address();
                                    tx.send((raw::Stream::Quic(socket), PeerAddr::Quic(addr)))
                                        .await
                                        .ok();
                                }
                                Err(error) => {
                                    tracing::error!(?error, "Failed to accept connection");
                                }
                            };
                        }
                        connectings.lock().unwrap().remove(&connecting_id);
                    }
                });

                connectings.lock().unwrap().insert(connecting_id, task);
                start_task_tx.send(()).unwrap_or(());
            }
            None => {
                tracing::error!("Stopped accepting new connections");
                break;
            }
        }
    }
}

// Filter out some weird `SocketAddr`s. We don't want to connect to those.
fn ok_to_connect(addr: &SocketAddr, source: PeerSource) -> bool {
    if addr.port() == 0 || addr.port() == 1 {
        return false;
    }

    match addr {
        SocketAddr::V4(addr) => {
            let ip_addr = addr.ip();
            if ip_addr.octets()[0] == 0 {
                return false;
            }
            if ip::is_benchmarking(ip_addr)
                || ip::is_reserved(ip_addr)
                || ip_addr.is_broadcast()
                || ip_addr.is_documentation()
            {
                return false;
            }

            if source == PeerSource::Dht
                && (ip_addr.is_private() || ip_addr.is_loopback() || ip_addr.is_link_local())
            {
                return false;
            }
        }
        SocketAddr::V6(addr) => {
            let ip_addr = addr.ip();

            if ip_addr.is_multicast() || ip_addr.is_unspecified() || ip::is_documentation(ip_addr) {
                return false;
            }

            if source == PeerSource::Dht
                && (ip_addr.is_loopback()
                    || ip::is_unicast_link_local(ip_addr)
                    || ip::is_unique_local(ip_addr))
            {
                return false;
            }
        }
    }

    true
}

#[derive(Debug)]
pub(super) struct StackAddresses {
    quic_v4: Option<SocketAddr>,
    quic_v6: Option<SocketAddr>,
    tcp_v4: Option<SocketAddr>,
    tcp_v6: Option<SocketAddr>,
}

impl StackAddresses {
    pub(super) fn any_stack_needs_rebind(&self, new_stack_addresses: &StackAddresses) -> bool {
        needs_rebind(&self.quic_v4, &new_stack_addresses.quic_v4)
            || needs_rebind(&self.quic_v6, &new_stack_addresses.quic_v6)
            || needs_rebind(&self.tcp_v4, &new_stack_addresses.tcp_v4)
            || needs_rebind(&self.tcp_v6, &new_stack_addresses.tcp_v6)
    }

    fn ip_addrs(&self) -> Vec<IpAddr> {
        self.quic_v4
            .iter()
            .chain(self.quic_v6.iter())
            .chain(self.tcp_v4.iter())
            .chain(self.tcp_v6.iter())
            .map(|addr| addr.ip())
            .collect()
    }
}

fn needs_rebind(old_addr: &Option<SocketAddr>, new_addr: &Option<SocketAddr>) -> bool {
    match (old_addr, new_addr) {
        (Some(old_addr), Some(new_addr)) => {
            let old_ip = old_addr.ip();
            let old_port = old_addr.port();
            let new_ip = new_addr.ip();
            let new_port = new_addr.port();

            // Just for readability as "true" and "false" have different lengths.
            const T: bool = true;
            const F: bool = false;

            // `old_port` is not expected to be 0, but doesn't hurt to cover that case as well.
            match (
                old_ip.is_unspecified(),
                old_port == 0,
                new_ip.is_unspecified(),
                new_port == 0,
            ) {
                (T, T, T, T) => false,
                (F, T, T, T) => true,
                (T, F, T, T) => false,
                (F, F, T, T) => true,
                (T, T, F, T) => true,
                (F, T, F, T) => old_ip != new_ip,
                (T, F, F, T) => true,
                (F, F, F, T) => old_ip != new_ip,
                (T, T, T, F) => true,
                (F, T, T, F) => true,
                (T, F, T, F) => old_port != new_port,
                (F, F, T, F) => true,
                (T, T, F, F) => true,
                (F, T, F, F) => true,
                (T, F, F, F) => true,
                (F, F, F, F) => old_ip != new_ip || old_port != new_port,
            }
        }
        (Some(_), None) => true,
        (None, Some(_)) => true,
        (None, None) => false,
    }
}

impl From<&[PeerAddr]> for StackAddresses {
    fn from(addrs: &[PeerAddr]) -> Self {
        let quic_v4 = addrs.iter().find_map(|addr| match addr {
            PeerAddr::Quic(addr @ SocketAddr::V4(_)) => Some(*addr),
            _ => None,
        });
        let quic_v6 = addrs.iter().find_map(|addr| match addr {
            PeerAddr::Quic(addr @ SocketAddr::V6(_)) => Some(*addr),
            _ => None,
        });
        let tcp_v4 = addrs.iter().find_map(|addr| match addr {
            PeerAddr::Tcp(addr @ SocketAddr::V4(_)) => Some(*addr),
            _ => None,
        });
        let tcp_v6 = addrs.iter().find_map(|addr| match addr {
            PeerAddr::Tcp(addr @ SocketAddr::V6(_)) => Some(*addr),
            _ => None,
        });

        StackAddresses {
            quic_v4,
            quic_v6,
            tcp_v4,
            tcp_v6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Connectivity {
    Disabled,
    LocalOnly,
    Full,
}

impl Connectivity {
    // `addrs` are the local addresses we're binding to.
    pub fn infer(addrs: &[IpAddr]) -> Self {
        if addrs.is_empty() {
            return Self::Disabled;
        }

        let global = addrs
            .iter()
            .any(|ip| ip.is_unspecified() || ip::is_global(ip));

        if global {
            Self::Full
        } else {
            Self::LocalOnly
        }
    }

    pub fn allows_connection_to(&self, addr: PeerAddr) -> bool {
        match self {
            Self::Disabled => false,
            Self::LocalOnly => !ip::is_global(&addr.ip()),
            Self::Full => true,
        }
    }

    // `Full` is_less_restrictive_than `LocalOnly` is_less_restrictive_than `Disabled`.
    pub fn is_less_restrictive_than(&self, other: Connectivity) -> bool {
        match (self, other) {
            (Self::LocalOnly, Self::Disabled)
            | (Self::Full, Self::Disabled)
            | (Self::Full, Self::LocalOnly) => true,
            (_, _) => false,
        }
    }
}
