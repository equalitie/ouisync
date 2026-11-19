use super::{
    bind_addr::{BindAddr, BindIp, BindPort},
    ip,
    peer_addr::PeerAddr,
    seen_peers::SeenPeer,
};
use backoff::{ExponentialBackoffBuilder, backoff::Backoff};
use deadlock::BlockingMutex;
use futures_util::{
    FutureExt,
    future::{self, BoxFuture, Either},
};
use net::{
    SocketOptions, quic, tcp,
    unified::{Acceptor, Connection},
};
use portable_atomic::{AtomicU64, Ordering};
use scoped_task::ScopedJoinHandle;
use std::{
    net::{IpAddr, SocketAddr},
    num::NonZeroU16,
};
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
    // Addresses the stacks should be bound to. Can differ from the actual addresses of `stacks`
    // when binding a stack failed or when a stack failed later.
    wanted: watch::Sender<StackBindAddrs>,
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
            wanted: watch::Sender::new(StackBindAddrs::default()),
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
        self.wanted.send_replace(*bind);

        // Close the current stacks before binding the new ones, so the new ones can be bound to the
        // same ports. The closed stacks stay in place until replaced below, so that the observers
        // don't see a temporary unbound state.
        let closer = self.stacks.borrow().closer(StackSlot::ALL);
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

        drop(next);
        self.update_wanted_ports();
    }

    /// Closes the stack in the given slot (if any) and binds a new one to the wanted address. The
    /// other stacks are left alone. Returns whether the slot now has a working stack.
    pub async fn rebind_stack(&self, slot: StackSlot) -> bool {
        let Some(bind_addr) = self.wanted.borrow().get(slot) else {
            return false;
        };

        // Close the current stack first, so the new one can be bound to the same port.
        let closer = self.stacks.borrow().closer([slot]);
        closer.close().await;

        let bound = match slot {
            StackSlot::QuicV4 | StackSlot::QuicV6 => {
                let stack = QuicStack::new(bind_addr, self.incoming_tx.clone());
                let bound = stack.is_some();

                // Nothing to change (no stack before, still no stack now). Don't notify the
                // observers for nothing.
                if !bound && self.stacks.borrow().quic(slot).is_none() {
                    return false;
                }

                self.stacks.send_modify(|stacks| {
                    *stacks.quic_mut(slot) = stack;
                    stacks.id = Stacks::next_id();
                });

                bound
            }
            StackSlot::TcpV4 | StackSlot::TcpV6 => {
                let stack = TcpStack::new(bind_addr, self.incoming_tx.clone());
                let bound = stack.is_some();

                // See above.
                if !bound && self.stacks.borrow().tcp(slot).is_none() {
                    return false;
                }

                self.stacks.send_modify(|stacks| {
                    *stacks.tcp_mut(slot) = stack;
                    stacks.id = Stacks::next_id();
                });

                bound
            }
        };

        if bound {
            tracing::info!(?slot, "Rebound stack");
            self.update_wanted_ports();
        }

        bound
    }

    /// Slots of the stacks that are wanted but either not bound or failed.
    pub fn missing_stacks(&self) -> Vec<StackSlot> {
        let wanted = self.wanted.borrow();
        let stacks = self.stacks.borrow();

        StackSlot::ALL
            .into_iter()
            .filter(|slot| wanted.get(*slot).is_some() && !stacks.is_working(*slot))
            .collect()
    }

    /// Waits until some wanted stack is missing (see `missing_stacks`).
    pub async fn wait_for_missing_stack(&self) {
        let mut stacks_rx = self.stacks.subscribe();
        let mut wanted_rx = self.wanted.subscribe();

        loop {
            if !self.missing_stacks().is_empty() {
                return;
            }

            let on_failure = self.stacks.borrow().on_any_failure();

            select! {
                _ = on_failure => (),
                _ = stacks_rx.changed() => (),
                _ = wanted_rx.changed() => (),
            }
        }
    }

    /// Notifies when the wanted addresses change (that is, on `bind`).
    pub fn subscribe_wanted(&self) -> watch::Receiver<StackBindAddrs> {
        self.wanted.subscribe()
    }

    /// Id of the current stacks. Changes whenever the stacks change.
    pub fn stacks_id(&self) -> u64 {
        self.stacks.borrow().id
    }

    #[cfg(test)]
    pub fn kill_quic_stack(&self, slot: StackSlot) {
        self.stacks.borrow().quic(slot).unwrap().connector.kill();
    }

    /// Updates the wanted ports of the stacks bound to a random or preferred port to their actual
    /// ports, so that they are bound to the same ports again when rebound.
    fn update_wanted_ports(&self) {
        let stacks = self.stacks.borrow();

        self.wanted.send_if_modified(|wanted| {
            let mut modified = false;

            for slot in StackSlot::ALL {
                let Some(wanted_addr) = wanted.get_mut(slot) else {
                    continue;
                };

                if matches!(wanted_addr.port, BindPort::Strict(_)) {
                    continue;
                }

                let Some(port) = stacks
                    .listener_local_addr(slot)
                    .and_then(|addr| NonZeroU16::new(addr.port()))
                else {
                    continue;
                };

                if wanted_addr.port != BindPort::Preferred(port) {
                    wanted_addr.port = BindPort::Preferred(port);
                    modified = true;
                }
            }

            modified
        });
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

        let mut hole_punching = HolePunching::default();
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
            let (stacks_id, connect) = {
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

                hole_punching.update(&stacks, addr);

                (stacks.id, stacks.connect(addr))
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
                        // The connector has been closed because its stack is being replaced (see
                        // `Gateway::bind`). Wait for the replacement and try again with it.
                        let replaced = select! {
                            result = stacks_rx.wait_for(|stacks| stacks.id != stacks_id) => {
                                result.is_ok()
                            }
                            _ = peer.on_unseen() => false,
                        };

                        if replaced {
                            continue;
                        } else {
                            return None;
                        }
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
    // Unique id of this set of stacks. Used to detect when the stacks change.
    id: u64,
    quic_v4: Option<QuicStack>,
    quic_v6: Option<QuicStack>,
    tcp_v4: Option<TcpStack>,
    tcp_v6: Option<TcpStack>,
}

impl Stacks {
    fn next_id() -> u64 {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    }

    fn unbound() -> Self {
        Self {
            id: Self::next_id(),
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
            id: Self::next_id(),
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
            || self.tcp_v4.as_ref().is_some_and(TcpStack::has_failed)
            || self.tcp_v6.as_ref().is_some_and(TcpStack::has_failed)
    }

    /// Completes when any of the stacks fails.
    fn on_any_failure(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut futures: Vec<BoxFuture<'static, ()>> = Vec::new();

        for stack in [&self.quic_v4, &self.quic_v6].into_iter().flatten() {
            futures.push(stack.connector.on_failure().boxed());
        }

        for stack in [&self.tcp_v4, &self.tcp_v6].into_iter().flatten() {
            let mut failed_rx = stack.failed_tx.subscribe();

            futures.push(
                async move {
                    if failed_rx.wait_for(|failed| *failed).await.is_err() {
                        // The stack has been dropped without failing.
                        future::pending().await
                    }
                }
                .boxed(),
            );
        }

        async move {
            if futures.is_empty() {
                future::pending().await
            } else {
                future::select_all(futures).await;
            }
        }
    }

    /// Whether the slot has a bound stack that hasn't failed.
    fn is_working(&self, slot: StackSlot) -> bool {
        match slot {
            StackSlot::QuicV4 | StackSlot::QuicV6 => {
                self.quic(slot).is_some_and(|stack| !stack.has_failed())
            }
            StackSlot::TcpV4 | StackSlot::TcpV6 => {
                self.tcp(slot).is_some_and(|stack| !stack.has_failed())
            }
        }
    }

    fn listener_local_addr(&self, slot: StackSlot) -> Option<SocketAddr> {
        match slot {
            StackSlot::QuicV4 | StackSlot::QuicV6 => {
                self.quic(slot).map(|stack| stack.listener_local_addr)
            }
            StackSlot::TcpV4 | StackSlot::TcpV6 => {
                self.tcp(slot).map(|stack| stack.listener_local_addr)
            }
        }
    }

    fn quic(&self, slot: StackSlot) -> Option<&QuicStack> {
        match slot {
            StackSlot::QuicV4 => self.quic_v4.as_ref(),
            StackSlot::QuicV6 => self.quic_v6.as_ref(),
            StackSlot::TcpV4 | StackSlot::TcpV6 => None,
        }
    }

    fn tcp(&self, slot: StackSlot) -> Option<&TcpStack> {
        match slot {
            StackSlot::TcpV4 => self.tcp_v4.as_ref(),
            StackSlot::TcpV6 => self.tcp_v6.as_ref(),
            StackSlot::QuicV4 | StackSlot::QuicV6 => None,
        }
    }

    fn quic_mut(&mut self, slot: StackSlot) -> &mut Option<QuicStack> {
        match slot {
            StackSlot::QuicV4 => &mut self.quic_v4,
            StackSlot::QuicV6 => &mut self.quic_v6,
            StackSlot::TcpV4 | StackSlot::TcpV6 => unreachable!("not a QUIC slot"),
        }
    }

    fn tcp_mut(&mut self, slot: StackSlot) -> &mut Option<TcpStack> {
        match slot {
            StackSlot::TcpV4 => &mut self.tcp_v4,
            StackSlot::TcpV6 => &mut self.tcp_v6,
            StackSlot::QuicV4 | StackSlot::QuicV6 => unreachable!("not a TCP slot"),
        }
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

    /// Returns a `StacksCloser` which closes the stacks in the given slots, releasing their ports.
    fn closer(&self, slots: impl IntoIterator<Item = StackSlot>) -> StacksCloser {
        let mut closer = StacksCloser {
            quic_connectors: Vec::new(),
            tcp_listener_tasks: Vec::new(),
        };

        for slot in slots {
            if let Some(stack) = self.quic(slot) {
                stack.listener_task.abort();
                closer.quic_connectors.push(stack.connector.clone());
            }

            if let Some(stack) = self.tcp(slot) {
                closer
                    .tcp_listener_tasks
                    .extend(stack.listener_task.lock().unwrap().take());
            }
        }

        closer
    }
}

/// Hole punching task for a single peer. Restarted when the stacks change, so it doesn't keep using
/// a replaced socket.
#[derive(Default)]
struct HolePunching {
    // The task and the id of the stacks it was started with.
    current: Option<(u64, ScopedJoinHandle<()>)>,
}

impl HolePunching {
    fn update(&mut self, stacks: &Stacks, addr: PeerAddr) {
        if self
            .current
            .as_ref()
            .is_some_and(|(stacks_id, _)| *stacks_id == stacks.id)
        {
            return;
        }

        // Dropping the previous task (if any) aborts it.
        self.current = stacks
            .start_punching_holes(addr)
            .map(|task| (stacks.id, task));
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
        // Failures of the QUIC listener are detected via the connector (see `QuicStack::has_failed`).
        let listener_task = scoped_task::spawn(
            async move {
                run_listener(Acceptor::Quic(acceptor), incoming_tx).await;
            }
            .instrument(span),
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
    // Set to `true` when the listener stops accepting connections because of an error.
    failed_tx: watch::Sender<bool>,
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
        let failed_tx = watch::Sender::new(false);
        let listener_task = scoped_task::spawn(
            run_tcp_listener(acceptor, incoming_tx, failed_tx.clone()).instrument(span),
        );

        Some(Self {
            listener_local_addr,
            listener_task: BlockingMutex::new(Some(listener_task)),
            connector,
            failed_tx,
        })
    }

    /// Whether the listener stopped accepting connections because of an error.
    fn has_failed(&self) -> bool {
        *self.failed_tx.borrow()
    }
}

/// Runs the TCP listener and marks it as failed (via `failed_tx`) if it stops because of an error.
async fn run_tcp_listener(
    acceptor: tcp::Acceptor,
    incoming_tx: mpsc::Sender<(Connection, PeerAddr)>,
    failed_tx: watch::Sender<bool>,
) {
    if run_listener(Acceptor::Tcp(acceptor), incoming_tx).await == ListenerExit::Failed {
        failed_tx.send_replace(true);
    }
}

/// Why `run_listener` finished.
#[derive(Eq, PartialEq, Debug)]
enum ListenerExit {
    /// Accepting failed.
    Failed,
    /// Nobody is receiving the incoming connections anymore.
    Closed,
}

async fn run_listener(
    listener: Acceptor,
    tx: mpsc::Sender<(Connection, PeerAddr)>,
) -> ListenerExit {
    let mut tasks = JoinSet::new();

    loop {
        let connecting = select! {
            connecting = listener.accept() => connecting,
            _ = tx.closed() => return ListenerExit::Closed,
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
                return ListenerExit::Failed;
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
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
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

impl StackBindAddrs {
    fn get(&self, slot: StackSlot) -> Option<StackBindAddr> {
        match slot {
            StackSlot::QuicV4 => self.quic_v4,
            StackSlot::QuicV6 => self.quic_v6,
            StackSlot::TcpV4 => self.tcp_v4,
            StackSlot::TcpV6 => self.tcp_v6,
        }
    }

    fn get_mut(&mut self, slot: StackSlot) -> Option<&mut StackBindAddr> {
        match slot {
            StackSlot::QuicV4 => self.quic_v4.as_mut(),
            StackSlot::QuicV6 => self.quic_v6.as_mut(),
            StackSlot::TcpV4 => self.tcp_v4.as_mut(),
            StackSlot::TcpV6 => self.tcp_v6.as_mut(),
        }
    }
}

/// Address to bind a single stack to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

/// Identifies one of the (up to four) stacks.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum StackSlot {
    QuicV4,
    QuicV6,
    TcpV4,
    TcpV6,
}

impl StackSlot {
    pub const ALL: [Self; 4] = [Self::QuicV4, Self::QuicV6, Self::TcpV4, Self::TcpV6];
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
    use crate::network::seen_peers::SeenPeers;
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

    #[tokio::test]
    async fn hole_punching_restarts_when_stacks_change() {
        let gateway = Gateway::new(mpsc::channel(1).0);
        gateway
            .bind(&StackBindAddrs::from(&[quic_addr(BindPort::Random)][..]))
            .await;

        // Hole punching is only done for global addresses. The task sleeps before sending anything,
        // so no packets are actually sent during this test.
        let peer_addr = PeerAddr::Quic((Ipv4Addr::new(1, 1, 1, 1), 1234).into());
        let task_id = |hole_punching: &HolePunching| {
            hole_punching
                .current
                .as_ref()
                .map(|(_, task)| task.0.id())
                .unwrap()
        };

        let mut hole_punching = HolePunching::default();
        hole_punching.update(&gateway.stacks.borrow(), peer_addr);
        let first_task_id = task_id(&hole_punching);

        // Same stacks - not restarted.
        hole_punching.update(&gateway.stacks.borrow(), peer_addr);
        assert_eq!(task_id(&hole_punching), first_task_id);

        // Stacks changed - restarted.
        gateway
            .bind(&StackBindAddrs::from(&[quic_addr(BindPort::Random)][..]))
            .await;
        hole_punching.update(&gateway.stacks.borrow(), peer_addr);
        assert_ne!(task_id(&hole_punching), first_task_id);
    }

    #[tokio::test]
    async fn connect_survives_rebind() {
        let gateway = Gateway::new(mpsc::channel(1).0);
        gateway
            .bind(&StackBindAddrs::from(&[quic_addr(BindPort::Random)][..]))
            .await;

        // Reserve a port where nobody answers yet, so the first connection attempt is still in
        // progress when the gateway gets rebound.
        let silent = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let peer_addr = silent.local_addr().unwrap();

        let seen_peers = SeenPeers::new();
        let peer = seen_peers.insert(PeerAddr::Quic(peer_addr)).unwrap();

        // Timeout for the whole thing because if the connecting side gives up, the accepting side
        // would wait forever.
        let (connection, _accepted) = time::timeout(Duration::from_secs(10), async {
            tokio::join!(gateway.connect_with_retries(&peer), async {
                time::sleep(Duration::from_millis(200)).await;

                gateway
                    .bind(&StackBindAddrs::from(&[quic_addr(BindPort::Random)][..]))
                    .await;

                // Now start answering.
                drop(silent);
                let (connector, acceptor, _) =
                    quic::configure(peer_addr, SocketOptions::default().with_strict_port())
                        .unwrap();
                let connection = acceptor.accept().await.unwrap().await.unwrap();

                // Keep everything alive until the end of the test.
                (connector, acceptor, connection)
            })
        })
        .await
        .unwrap();

        assert!(connection.is_some());
    }

    #[tokio::test]
    async fn rebind_only_the_failed_stack() {
        let (incoming_tx, mut incoming_rx) = mpsc::channel(1);
        let gateway = Gateway::new(incoming_tx);

        gateway
            .bind(&StackBindAddrs::from(
                &[quic_addr(BindPort::Random), tcp_addr(BindPort::Random)][..],
            ))
            .await;
        let quic_port = bound_quic_port(&gateway).unwrap();
        let tcp_port = bound_tcp_port(&gateway).unwrap();

        // Open a TCP connection which should survive rebinding the QUIC stack.
        let _client = std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, tcp_port)).unwrap();
        let (accepted, _) = incoming_rx.recv().await.unwrap();

        let tcp_listener_task_id = |gateway: &Gateway| {
            gateway
                .stacks
                .borrow()
                .tcp_v4
                .as_ref()
                .unwrap()
                .listener_task
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .0
                .id()
        };
        let tcp_task_id = tcp_listener_task_id(&gateway);

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

        assert_eq!(gateway.missing_stacks(), [StackSlot::QuicV4]);

        assert!(gateway.rebind_stack(StackSlot::QuicV4).await);
        assert_eq!(gateway.missing_stacks(), []);
        assert_eq!(bound_quic_port(&gateway), Some(quic_port));
        assert_eq!(bound_tcp_port(&gateway), Some(tcp_port));

        // The TCP stack wasn't touched and the TCP connection is still open.
        assert_eq!(tcp_listener_task_id(&gateway), tcp_task_id);
        assert!(
            time::timeout(Duration::from_millis(200), accepted.closed())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn rebind_strict_port_taken() {
        let taken = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let taken_port = taken.local_addr().unwrap().port();

        let gateway = Gateway::new(mpsc::channel(1).0);
        gateway
            .bind(&StackBindAddrs::from(
                &[quic_addr(BindPort::Strict(port(taken_port)))][..],
            ))
            .await;
        assert_eq!(gateway.missing_stacks(), [StackSlot::QuicV4]);

        // Still taken
        assert!(!gateway.rebind_stack(StackSlot::QuicV4).await);
        assert_eq!(gateway.missing_stacks(), [StackSlot::QuicV4]);
        assert_eq!(bound_quic_port(&gateway), None);

        // Free
        drop(taken);
        assert!(gateway.rebind_stack(StackSlot::QuicV4).await);
        assert_eq!(gateway.missing_stacks(), []);
        assert_eq!(bound_quic_port(&gateway), Some(taken_port));
    }

    #[tokio::test]
    async fn wanted_port_follows_actual_port() {
        let taken = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let taken_port = taken.local_addr().unwrap().port();

        let gateway = Gateway::new(mpsc::channel(1).0);
        gateway
            .bind(&StackBindAddrs::from(
                &[
                    quic_addr(BindPort::Preferred(port(taken_port))),
                    tcp_addr(BindPort::Random),
                ][..],
            ))
            .await;

        let quic_port = bound_quic_port(&gateway).unwrap();
        let tcp_port = bound_tcp_port(&gateway).unwrap();
        assert_ne!(quic_port, taken_port);

        let wanted = *gateway.wanted.borrow();
        assert_eq!(
            wanted.get(StackSlot::QuicV4).unwrap().port,
            BindPort::Preferred(port(quic_port))
        );
        assert_eq!(
            wanted.get(StackSlot::TcpV4).unwrap().port,
            BindPort::Preferred(port(tcp_port))
        );
    }

    #[cfg(target_os = "linux")] // See `tcp::Acceptor::kill`.
    #[tokio::test]
    async fn tcp_listener_failure_is_detected() {
        let (_connector, acceptor) =
            tcp::configure((Ipv4Addr::LOCALHOST, 0).into(), SocketOptions::default()).unwrap();
        acceptor.kill();

        let failed_tx = watch::Sender::new(false);
        time::timeout(
            Duration::from_secs(5),
            run_tcp_listener(acceptor, mpsc::channel(1).0, failed_tx.clone()),
        )
        .await
        .unwrap();
        assert!(*failed_tx.borrow());
    }

    #[tokio::test]
    async fn failed_tcp_stack_is_missing_and_rebindable() {
        let gateway = Gateway::new(mpsc::channel(1).0);
        gateway
            .bind(&StackBindAddrs::from(&[tcp_addr(BindPort::Random)][..]))
            .await;
        let tcp_port = bound_tcp_port(&gateway).unwrap();

        gateway
            .stacks
            .borrow()
            .tcp_v4
            .as_ref()
            .unwrap()
            .failed_tx
            .send_replace(true);
        assert_eq!(gateway.missing_stacks(), [StackSlot::TcpV4]);

        assert!(gateway.rebind_stack(StackSlot::TcpV4).await);
        assert_eq!(gateway.missing_stacks(), []);
        assert_eq!(bound_tcp_port(&gateway), Some(tcp_port));
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
