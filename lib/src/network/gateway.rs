use super::{
    bind_addr::{BindAddr, BindIp, BindPort},
    ip,
    peer_addr::PeerAddr,
    seen_peers::SeenPeer,
};
use backoff::{ExponentialBackoffBuilder, backoff::Backoff};
use deadlock::BlockingMutex;
use futures_util::future::{self, Either};
use net::{
    SocketOptions, quic, tcp,
    unified::{Acceptor, Connection},
};
use scoped_task::ScopedJoinHandle;
use std::net::{IpAddr, SocketAddr};
use thiserror::Error;
use tokio::{
    select,
    sync::{mpsc, watch},
    task::JoinSet,
    time::{self, Duration},
};
use tracing::{Instrument, Span, field};

/// Established incoming and outgoing connections.
pub(super) struct Gateway {
    stacks: watch::Sender<Stacks>,
    incoming_tx: mpsc::Sender<(Connection, PeerAddr)>,
}

impl Gateway {
    /// Create a new `Gateway` that is initially disabled.
    ///
    /// `incoming_tx` is the sender for the incoming connections.
    pub fn new(incoming_tx: mpsc::Sender<(Connection, PeerAddr)>) -> Self {
        let stacks = Stacks::unbound();
        let stacks = watch::Sender::new(stacks);

        Self {
            stacks,
            incoming_tx,
        }
    }

    pub fn listener_local_addrs(&self) -> Vec<PeerAddr> {
        let stacks = self.stacks.borrow();
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
    pub async fn bind(&self, bind: &StackBindAddrs) {
        // Close the current stacks before binding the new ones, so the new ones can be bound to the
        // same ports. The closed stacks stay in place until replaced below, so that the observers
        // don't see a temporary unbound state.
        let closer = self.stacks.borrow().closer();
        closer.close().await;

        let next = Stacks::bind(bind, self.incoming_tx.clone());

        let prev = self.stacks.send_replace(next);
        let next = self.stacks.borrow();

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

        let prev_conn = prev.connectivity();
        let next_conn = next.connectivity();

        if prev_conn != next_conn {
            tracing::info!(
                "Changed connectivity from {:?} to {:?}",
                prev_conn,
                next_conn
            );
        }
    }

    pub fn connectivity(&self) -> Connectivity {
        self.stacks.borrow().connectivity()
    }

    /// Whether any currently bound stack has hit a fatal error and stopped working. See
    /// `Stacks::any_stack_has_failed`.
    pub fn any_stack_has_failed(&self) -> bool {
        self.stacks.borrow().any_stack_has_failed()
    }

    pub async fn connect_with_retries(&self, peer: &SeenPeer) -> Option<Connection> {
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
        let mut prev_conn = Connectivity::Disabled;
        let mut stacks_rx = self.stacks.subscribe();

        loop {
            // Note: This needs to be probed each time the loop starts. When the `addr` fn returns
            // `None` that means whatever discovery mechanism (LocalDiscovery or DhtDiscovery)
            // found it is no longer seeing it.
            let addr = *peer.addr_if_seen()?;

            // Wait for a connector matching the address (in protocol and ip version) to become
            // available. Also wait for `Connectivity` to be such that it allows us to connect.
            // E.g. if it's `Connectivity::LocalOnly` then we can't connect to global addresses.
            let wait_for_connectivity =
                stacks_rx.wait_for(|stacks| stacks.allows_connection_to(&addr));

            // Ensure `stacks` is not borrowed across `await`.
            let connect = {
                let stacks = select! {
                    result = wait_for_connectivity => {
                        // unwrap is OK here because `self.stacks` still lives.
                        result.unwrap()
                    },
                    _ = peer.on_unseen() => return None,
                };

                let next_conn = stacks.connectivity();

                if next_conn.is_less_restrictive_than(prev_conn) {
                    backoff = create_backoff();
                }

                prev_conn = next_conn;

                if hole_punching_task.is_none() {
                    hole_punching_task = stacks.start_punching_holes(addr);
                }

                stacks.connect(addr)
            };

            match connect.await {
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
                    if !stacks_rx.borrow().allows_connection_to(&addr) {
                        continue;
                    }

                    return Some(socket);
                }
                Err(error) => {
                    if error.is_localy_closed() {
                        // Connector locally closed - no point in retrying.
                        return None;
                    }

                    match backoff.next_backoff() {
                        Some(duration) => {
                            tracing::trace!(
                                ?error,
                                "Connection failed. Next attempt in {:?}",
                                duration
                            );
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
        self.stacks.borrow().addresses()
    }

    pub fn udp_side_channel_maker_v4(&self) -> Option<quic::SideChannelMaker> {
        self.stacks
            .borrow()
            .quic_v4
            .as_ref()
            .map(|stack| &stack.side_channel_maker)
            .cloned()
    }

    pub fn udp_side_channel_maker_v6(&self) -> Option<quic::SideChannelMaker> {
        self.stacks
            .borrow()
            .quic_v6
            .as_ref()
            .map(|stack| &stack.side_channel_maker)
            .cloned()
    }
}

#[derive(Debug, Error)]
pub(super) enum ConnectError {
    #[error("TCP error")]
    Tcp(tcp::Error),
    #[error("QUIC error")]
    Quic(quic::Error),
    #[error("No corresponding connector")]
    NoSuitableConnector,
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

    fn bind(bind: &StackBindAddrs, incoming_tx: mpsc::Sender<(Connection, PeerAddr)>) -> Self {
        let quic_v4 = bind
            .quic_v4
            .and_then(|addr| QuicStack::new(addr, incoming_tx.clone()));

        let quic_v6 = bind
            .quic_v6
            .and_then(|addr| QuicStack::new(addr, incoming_tx.clone()));

        let tcp_v4 = if let Some(addr) = bind.tcp_v4 {
            TcpStack::new(addr, incoming_tx.clone())
        } else {
            None
        };

        let tcp_v6 = if let Some(addr) = bind.tcp_v6 {
            TcpStack::new(addr, incoming_tx)
        } else {
            None
        };

        Self {
            quic_v4,
            quic_v6,
            tcp_v4,
            tcp_v6,
        }
    }

    fn addresses(&self) -> StackAddresses {
        StackAddresses {
            quic_v4: self.quic_v4.as_ref().map(|stack| stack.listener_local_addr),
            quic_v6: self.quic_v6.as_ref().map(|stack| stack.listener_local_addr),
            tcp_v4: self.tcp_v4.as_ref().map(|stack| stack.listener_local_addr),
            tcp_v6: self.tcp_v6.as_ref().map(|stack| stack.listener_local_addr),
        }
    }

    fn connectivity(&self) -> Connectivity {
        Connectivity::infer(&self.addresses())
    }

    /// Whether any currently bound stack has hit a fatal error and stopped working (e.g. the
    /// network interface going down while the app is backgrounded). Such a stack never recovers
    /// on its own - it must be rebound, even if the address it was bound to hasn't changed.
    fn any_stack_has_failed(&self) -> bool {
        self.quic_v4.as_ref().is_some_and(QuicStack::has_failed)
            || self.quic_v6.as_ref().is_some_and(QuicStack::has_failed)
    }

    fn allows_connection_to(&self, addr: &PeerAddr) -> bool {
        let has_connector = match addr {
            PeerAddr::Tcp(SocketAddr::V4(_)) => self.tcp_v4.is_some(),
            PeerAddr::Tcp(SocketAddr::V6(_)) => self.tcp_v6.is_some(),
            PeerAddr::Quic(SocketAddr::V4(_)) => self.quic_v4.is_some(),
            PeerAddr::Quic(SocketAddr::V6(_)) => self.quic_v6.is_some(),
        };

        has_connector && self.connectivity().allows_connection_to(addr.ip())
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

    fn connect(
        &self,
        addr: PeerAddr,
    ) -> impl Future<Output = Result<Connection, ConnectError>> + Send + 'static {
        match addr {
            PeerAddr::Tcp(addr) => {
                let connect = self
                    .tcp_stack_for(&addr.ip())
                    .ok_or(ConnectError::NoSuitableConnector)
                    .map(|stack| stack.connector.connect(addr));
                let connect = async move {
                    connect?
                        .await
                        .map(Connection::Tcp)
                        .map_err(ConnectError::Tcp)
                };

                Either::Left(connect)
            }
            PeerAddr::Quic(addr) => {
                let connect = self
                    .quic_stack_for(&addr.ip())
                    .ok_or(ConnectError::NoSuitableConnector)
                    .map(|stack| stack.connector.connect(addr));
                let connect = async move {
                    connect?
                        .await
                        .map(Connection::Quic)
                        .map_err(ConnectError::Quic)
                };

                Either::Right(connect)
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

        let sender = stack.side_channel_maker.make().into_sender();
        let task = scoped_task::spawn(
            async move {
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
                    let duration = rand::random_range(5_000..15_000);
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

    fn tcp_stack_for(&self, ip: &IpAddr) -> Option<&TcpStack> {
        match ip {
            IpAddr::V4(_) => self.tcp_v4.as_ref(),
            IpAddr::V6(_) => self.tcp_v6.as_ref(),
        }
    }

    fn quic_stack_for(&self, ip: &IpAddr) -> Option<&QuicStack> {
        match ip {
            IpAddr::V4(_) => self.quic_v4.as_ref(),
            IpAddr::V6(_) => self.quic_v6.as_ref(),
        }
    }

    /// Returns a `StacksCloser` which closes all the stacks, releasing their ports.
    fn closer(&self) -> StacksCloser {
        let mut closer = StacksCloser {
            quic_connectors: Vec::new(),
            tcp_listener_tasks: Vec::new(),
        };

        for stack in [&self.quic_v4, &self.quic_v6].into_iter().flatten() {
            stack.listener_task.abort();
            closer.quic_connectors.push(stack.connector.clone());
        }

        for stack in [&self.tcp_v4, &self.tcp_v6].into_iter().flatten() {
            closer
                .tcp_listener_tasks
                .extend(stack.listener_task.lock().unwrap().take());
        }

        closer
    }
}

/// How long to wait for the peers to be notified about closed QUIC connections when closing a QUIC
/// stack.
const QUIC_GRACEFUL_CLOSE_TIMEOUT: Duration = Duration::from_millis(500);

struct StacksCloser {
    quic_connectors: Vec<quic::Connector>,
    tcp_listener_tasks: Vec<ScopedJoinHandle<()>>,
}

impl StacksCloser {
    async fn close(self) {
        future::join_all(self.quic_connectors.iter().map(|connector| async move {
            // A failed socket can't notify the peers anyway.
            if !connector.has_failed() {
                connector.graceful_close();
                time::timeout(QUIC_GRACEFUL_CLOSE_TIMEOUT, connector.wait_idle())
                    .await
                    .ok();
            }

            connector.hard_close();
        }))
        .await;

        // The TCP listener is owned by the listener task, so wait for the task to be dropped to
        // make sure the listener is closed.
        for task in self.tcp_listener_tasks {
            task.abort();
            task.await.ok();
        }
    }
}

struct QuicStack {
    listener_local_addr: SocketAddr,
    listener_task: ScopedJoinHandle<()>,
    connector: quic::Connector,
    side_channel_maker: quic::SideChannelMaker,
}

impl QuicStack {
    fn new(
        bind_addr: StackBindAddr,
        incoming_tx: mpsc::Sender<(Connection, PeerAddr)>,
    ) -> Option<Self> {
        let span = tracing::info_span!("quic", addr = field::Empty);

        let (connector, acceptor, side_channel_maker) =
            match quic::configure(bind_addr.socket_addr(), bind_addr.socket_options()) {
                Ok((connector, acceptor, side_channel_maker)) => {
                    span.record(
                        "addr",
                        field::display(PeerAddr::Quic(*acceptor.local_addr())),
                    );
                    tracing::info!(parent: &span, "Stack configured");

                    (connector, acceptor, side_channel_maker)
                }
                Err(error) => {
                    tracing::warn!(
                        parent: &span,
                        bind_addr = %PeerAddr::Quic(bind_addr.socket_addr()),
                        bind_port = ?bind_addr.port,
                        ?error,
                        "Failed to configure stack"
                    );
                    return None;
                }
            };

        let listener_local_addr = *acceptor.local_addr();
        let listener_task = scoped_task::spawn(
            run_listener(Acceptor::Quic(acceptor), incoming_tx).instrument(span),
        );

        Some(Self {
            connector,
            listener_local_addr,
            listener_task,
            side_channel_maker,
        })
    }

    fn has_failed(&self) -> bool {
        self.connector.has_failed()
    }
}

struct TcpStack {
    listener_local_addr: SocketAddr,
    // In `Option` so it can be taken out (see `Stacks::closer`).
    listener_task: BlockingMutex<Option<ScopedJoinHandle<()>>>,
    connector: tcp::Connector,
}

impl TcpStack {
    fn new(
        bind_addr: StackBindAddr,
        incoming_tx: mpsc::Sender<(Connection, PeerAddr)>,
    ) -> Option<Self> {
        let span = tracing::info_span!("tcp", addr = field::Empty);

        let (connector, acceptor) = match tcp::configure(
            bind_addr.socket_addr(),
            // Allows binding to the same port again while the previous socket's connections
            // are in `TIME_WAIT`.
            bind_addr.socket_options().with_reuse_addr(),
        ) {
            Ok((connector, acceptor)) => {
                span.record(
                    "addr",
                    field::display(PeerAddr::Tcp(*acceptor.local_addr())),
                );
                tracing::info!(parent: &span, "Stack configured");

                (connector, acceptor)
            }
            Err(error) => {
                tracing::warn!(
                    parent: &span,
                    bind_addr = %PeerAddr::Tcp(bind_addr.socket_addr()),
                    bind_port = ?bind_addr.port,
                    ?error,
                    "Failed to configure stack",
                );
                return None;
            }
        };

        let listener_local_addr = *acceptor.local_addr();
        let listener_task =
            scoped_task::spawn(run_listener(Acceptor::Tcp(acceptor), incoming_tx).instrument(span));

        Some(Self {
            listener_local_addr,
            listener_task: BlockingMutex::new(Some(listener_task)),
            connector,
        })
    }
}

async fn run_listener(listener: Acceptor, tx: mpsc::Sender<(Connection, PeerAddr)>) {
    let mut tasks = JoinSet::new();

    loop {
        let connecting = select! {
            connecting = listener.accept() => connecting,
            _ = tx.closed() => break,
        };

        match connecting {
            Ok(connecting) => {
                let tx = tx.clone();

                let addr = connecting.remote_addr();
                let addr = match listener {
                    Acceptor::Tcp(_) => PeerAddr::Tcp(addr),
                    Acceptor::Quic(_) => PeerAddr::Quic(addr),
                };

                // Spawn so we can start listening for the next connection ASAP.
                tasks.spawn(async move {
                    match connecting.await {
                        Ok(connection) => {
                            tx.send((connection, addr)).await.ok();
                        }
                        Err(error) => tracing::error!(?error, %addr, "Failed to accept connection"),
                    }
                });
            }
            Err(error) => {
                tracing::error!(?error, "Stopped accepting new connections");
                break;
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct StackAddresses {
    quic_v4: Option<SocketAddr>,
    quic_v6: Option<SocketAddr>,
    tcp_v4: Option<SocketAddr>,
    tcp_v6: Option<SocketAddr>,
}

impl StackAddresses {
    pub(super) fn any_stack_needs_rebind(&self, bind: &StackBindAddrs) -> bool {
        let required = |addr: &Option<StackBindAddr>| addr.map(|addr| addr.required_socket_addr());

        needs_rebind(&self.quic_v4, &required(&bind.quic_v4))
            || needs_rebind(&self.quic_v6, &required(&bind.quic_v6))
            || needs_rebind(&self.tcp_v4, &required(&bind.tcp_v4))
            || needs_rebind(&self.tcp_v6, &required(&bind.tcp_v6))
    }

    fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }

    fn iter(&self) -> impl Iterator<Item = &SocketAddr> {
        self.quic_v4
            .iter()
            .chain(&self.quic_v6)
            .chain(&self.tcp_v4)
            .chain(&self.tcp_v6)
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

/// Addresses to bind the stacks to.
#[derive(Debug)]
pub(super) struct StackBindAddrs {
    quic_v4: Option<StackBindAddr>,
    quic_v6: Option<StackBindAddr>,
    tcp_v4: Option<StackBindAddr>,
    tcp_v6: Option<StackBindAddr>,
}

impl From<&[BindAddr]> for StackBindAddrs {
    fn from(addrs: &[BindAddr]) -> Self {
        let find = |is_quic: bool, is_ipv4: bool| {
            addrs.iter().find_map(|addr| {
                let (ip, quic) = match addr.ip {
                    BindIp::Quic(ip) => (ip, true),
                    BindIp::Tcp(ip) => (ip, false),
                };

                (quic == is_quic && ip.is_ipv4() == is_ipv4).then_some(StackBindAddr {
                    ip,
                    port: addr.port,
                })
            })
        };

        Self {
            quic_v4: find(true, true),
            quic_v6: find(true, false),
            tcp_v4: find(false, true),
            tcp_v6: find(false, false),
        }
    }
}

/// Address to bind a single stack to.
#[derive(Clone, Copy, Debug)]
struct StackBindAddr {
    ip: IpAddr,
    port: BindPort,
}

impl StackBindAddr {
    /// Address to bind the socket to.
    fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.port.get())
    }

    fn socket_options(&self) -> SocketOptions {
        let options = SocketOptions::default();

        match self.port {
            BindPort::Strict(_) => options.with_strict_port(),
            BindPort::Preferred(_) | BindPort::Random => options,
        }
    }

    /// Address an already bound stack needs to have for it to not need rebinding. Port `0` means
    /// any port.
    fn required_socket_addr(&self) -> SocketAddr {
        let port = match self.port {
            BindPort::Strict(port) => port.get(),
            // Rebinding just to try the preferred port again isn't worth interrupting the
            // existing connections.
            BindPort::Preferred(_) | BindPort::Random => 0,
        };

        SocketAddr::new(self.ip, port)
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
    pub fn infer(addrs: &StackAddresses) -> Self {
        if addrs.is_empty() {
            return Self::Disabled;
        }

        let global = addrs
            .iter()
            .map(|addr| addr.ip())
            .any(|ip| ip.is_unspecified() || ip::is_global(&ip));

        if global { Self::Full } else { Self::LocalOnly }
    }

    pub fn allows_connection_to(&self, addr: IpAddr) -> bool {
        match self {
            Self::Disabled => false,
            Self::LocalOnly => !ip::is_global(&addr),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::Ipv4Addr, num::NonZeroU16};

    fn quic_addr(port: BindPort) -> BindAddr {
        BindAddr {
            ip: BindIp::Quic(Ipv4Addr::LOCALHOST.into()),
            port,
        }
    }

    fn tcp_addr(port: BindPort) -> BindAddr {
        BindAddr {
            ip: BindIp::Tcp(Ipv4Addr::LOCALHOST.into()),
            port,
        }
    }

    fn port(port: u16) -> NonZeroU16 {
        NonZeroU16::new(port).unwrap()
    }

    fn bound_quic_port(gateway: &Gateway) -> Option<u16> {
        gateway
            .listener_local_addrs()
            .into_iter()
            .find(|addr| addr.is_quic())
            .map(|addr| addr.port())
    }

    fn bound_tcp_port(gateway: &Gateway) -> Option<u16> {
        gateway
            .listener_local_addrs()
            .into_iter()
            .find(|addr| addr.is_tcp())
            .map(|addr| addr.port())
    }

    #[tokio::test]
    async fn bind_strict_port() {
        let taken = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let taken_port = taken.local_addr().unwrap().port();

        let gateway = Gateway::new(mpsc::channel(1).0);

        // Port taken
        gateway
            .bind(&StackBindAddrs::from(
                &[quic_addr(BindPort::Strict(port(taken_port)))][..],
            ))
            .await;
        assert_eq!(bound_quic_port(&gateway), None);

        // Port free
        drop(taken);
        gateway
            .bind(&StackBindAddrs::from(
                &[quic_addr(BindPort::Strict(port(taken_port)))][..],
            ))
            .await;
        assert_eq!(bound_quic_port(&gateway), Some(taken_port));
    }

    #[tokio::test]
    async fn bind_preferred_port() {
        let taken = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let taken_port = taken.local_addr().unwrap().port();

        let gateway = Gateway::new(mpsc::channel(1).0);

        // Port taken - falls back to a random one
        gateway
            .bind(&StackBindAddrs::from(
                &[quic_addr(BindPort::Preferred(port(taken_port)))][..],
            ))
            .await;
        let bound_port = bound_quic_port(&gateway).unwrap();
        assert_ne!(bound_port, taken_port);

        // Port free
        drop(taken);
        gateway
            .bind(&StackBindAddrs::from(
                &[quic_addr(BindPort::Preferred(port(taken_port)))][..],
            ))
            .await;
        assert_eq!(bound_quic_port(&gateway), Some(taken_port));
    }

    #[tokio::test]
    async fn rebind_to_same_strict_ports() {
        let gateway = Gateway::new(mpsc::channel(1).0);

        gateway
            .bind(&StackBindAddrs::from(
                &[quic_addr(BindPort::Random), tcp_addr(BindPort::Random)][..],
            ))
            .await;
        let quic_port = bound_quic_port(&gateway).unwrap();
        let tcp_port = bound_tcp_port(&gateway).unwrap();

        // The old stacks are closed before binding the new ones, so the ports are free.
        gateway
            .bind(&StackBindAddrs::from(
                &[
                    quic_addr(BindPort::Strict(port(quic_port))),
                    tcp_addr(BindPort::Strict(port(tcp_port))),
                ][..],
            ))
            .await;
        assert_eq!(bound_quic_port(&gateway), Some(quic_port));
        assert_eq!(bound_tcp_port(&gateway), Some(tcp_port));
    }

    #[tokio::test]
    async fn rebind_failed_quic_stack_to_same_port() {
        let gateway = Gateway::new(mpsc::channel(1).0);

        gateway
            .bind(&StackBindAddrs::from(&[quic_addr(BindPort::Random)][..]))
            .await;
        let quic_port = bound_quic_port(&gateway).unwrap();

        // A failed socket keeps its port until closed.
        let connector = gateway
            .stacks
            .borrow()
            .quic_v4
            .as_ref()
            .unwrap()
            .connector
            .clone();
        let on_failure = connector.on_failure();
        connector.kill();
        time::timeout(Duration::from_secs(5), on_failure)
            .await
            .unwrap();

        gateway
            .bind(&StackBindAddrs::from(
                &[quic_addr(BindPort::Strict(port(quic_port)))][..],
            ))
            .await;
        assert_eq!(bound_quic_port(&gateway), Some(quic_port));
    }

    #[tokio::test]
    async fn rebind_notifies_quic_peers() {
        let (incoming_tx, mut incoming_rx) = mpsc::channel(1);
        let gateway = Gateway::new(incoming_tx);

        gateway
            .bind(&StackBindAddrs::from(&[quic_addr(BindPort::Random)][..]))
            .await;
        let quic_port = bound_quic_port(&gateway).unwrap();

        let (client, _client_acceptor, _) =
            quic::configure((Ipv4Addr::LOCALHOST, 0).into(), SocketOptions::default()).unwrap();
        let connection = client
            .connect((Ipv4Addr::LOCALHOST, quic_port).into())
            .await
            .unwrap();
        let _accepted = incoming_rx.recv().await.unwrap();

        gateway
            .bind(&StackBindAddrs::from(
                &[quic_addr(BindPort::Strict(port(quic_port)))][..],
            ))
            .await;

        // The peer is notified about the closed connection instead of having to wait for the idle
        // timeout (20s).
        time::timeout(Duration::from_secs(5), connection.closed())
            .await
            .unwrap();
    }

    #[test]
    fn needs_rebind_by_bind_port() {
        let bound = StackAddresses {
            quic_v4: Some((Ipv4Addr::UNSPECIFIED, 5000).into()),
            quic_v6: None,
            tcp_v4: None,
            tcp_v6: None,
        };

        let check = |port| {
            bound.any_stack_needs_rebind(&StackBindAddrs::from(
                &[BindAddr {
                    ip: BindIp::Quic(Ipv4Addr::UNSPECIFIED.into()),
                    port,
                }][..],
            ))
        };

        assert!(!check(BindPort::Random));
        assert!(!check(BindPort::Preferred(port(5000))));
        // Not worth rebinding just to try a different preferred port.
        assert!(!check(BindPort::Preferred(port(6000))));
        assert!(!check(BindPort::Strict(port(5000))));
        assert!(check(BindPort::Strict(port(6000))));
    }
}
