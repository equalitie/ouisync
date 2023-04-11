use super::{ip, peer_addr::PeerAddr, peer_source::PeerSource, raw, seen_peers::SeenPeer};
use crate::sync::atomic_slot::AtomicSlot;
use backoff::{backoff::Backoff, ExponentialBackoffBuilder};
use net::{
    quic,
    tcp::{TcpListener, TcpStream},
};
use scoped_task::ScopedJoinHandle;
use std::net::{IpAddr, SocketAddr};
use thiserror::Error;
use tokio::{
    select,
    sync::mpsc,
    time::{self, Duration},
};
use tracing::{field, Instrument, Span};

/// Established incoming and outgoing connections.
pub(super) struct Gateway {
    stacks: AtomicSlot<Stacks>,
    incoming_tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
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
        bind: &[PeerAddr],
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

        prev.close();

        (side_channel_maker_v4, side_channel_maker_v6)
    }

    pub async fn connect_with_retries(
        &self,
        peer: &SeenPeer,
        source: PeerSource,
    ) -> Option<raw::Stream> {
        if !ok_to_connect(peer.addr_if_seen()?.socket_addr(), source) {
            tracing::warn!("invalid peer address - discarding");
            return None;
        }

        let mut backoff = ExponentialBackoffBuilder::new()
            .with_initial_interval(Duration::from_millis(200))
            .with_max_interval(Duration::from_secs(10))
            // We'll continue trying for as long as `peer.addr().is_some()`.
            .with_max_elapsed_time(None)
            .build();

        let mut hole_punching_task = None;

        loop {
            // Note: This needs to be probed each time the loop starts. When the `addr` fn returns
            // `None` that means whatever discovery mechanism (LocalDiscovery or DhtDiscovery)
            // found it is no longer seeing it.
            let addr = *peer.addr_if_seen()?;

            // Note: we need to grab fresh stacks on each loop because the network might get
            // re-bound in the meantime which would change the connectors.
            let stacks = self.stacks.read();

            if hole_punching_task.is_none() {
                hole_punching_task = stacks.start_punching_holes(addr);
            }

            match stacks.connect(addr).await {
                Ok(socket) => {
                    return Some(socket);
                }
                Err(error) => {
                    tracing::warn!("connection failed: {:?}", error);

                    if error.is_localy_closed() {
                        // Connector locally closed - no point in retrying.
                        return None;
                    }

                    match backoff.next_backoff() {
                        Some(duration) => {
                            tracing::debug!("next connection attempt in {:?}", duration);
                            time::sleep(duration).await;
                        }
                        // We set max elapsed time to None above.
                        None => unreachable!(),
                    }
                }
            }
        }
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
        bind: &[PeerAddr],
        incoming_tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
    ) -> (
        Self,
        Option<quic::SideChannelMaker>,
        Option<quic::SideChannelMaker>,
    ) {
        let bind_quic_v4 = bind.iter().find_map(|addr| match addr {
            PeerAddr::Quic(addr @ SocketAddr::V4(_)) => Some(*addr),
            _ => None,
        });

        let bind_quic_v6 = bind.iter().find_map(|addr| match addr {
            PeerAddr::Quic(addr @ SocketAddr::V6(_)) => Some(*addr),
            _ => None,
        });
        let bind_tcp_v4 = bind.iter().find_map(|addr| match addr {
            PeerAddr::Tcp(addr @ SocketAddr::V4(_)) => Some(*addr),
            _ => None,
        });
        let bind_tcp_v6 = bind.iter().find_map(|addr| match addr {
            PeerAddr::Tcp(addr @ SocketAddr::V6(_)) => Some(*addr),
            _ => None,
        });

        let (quic_v4, side_channel_maker_v4) = if let Some(addr) = bind_quic_v4 {
            QuicStack::new(addr, incoming_tx.clone())
                .await
                .map(|(stack, side_channel)| (Some(stack), Some(side_channel)))
                .unwrap_or((None, None))
        } else {
            (None, None)
        };

        let (quic_v6, side_channel_maker_v6) = if let Some(addr) = bind_quic_v6 {
            QuicStack::new(addr, incoming_tx.clone())
                .await
                .map(|(stack, side_channel)| (Some(stack), Some(side_channel)))
                .unwrap_or((None, None))
        } else {
            (None, None)
        };

        let tcp_v4 = if let Some(addr) = bind_tcp_v4 {
            TcpStack::new(addr, incoming_tx.clone()).await
        } else {
            None
        };

        let tcp_v6 = if let Some(addr) = bind_tcp_v6 {
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
            tracing::debug!("hole punching not started - not QUIC address");
            return None;
        }

        if !ip::is_global(&addr.ip()) {
            tracing::debug!("hole punching not started - not global address");
            return None;
        }

        let stack = if let Some(stack) = self.quic_stack_for(&addr.ip()) {
            stack
        } else {
            tracing::warn!("hole punching not started - no QUIC stack");
            return None;
        };

        let sender = stack.hole_puncher.clone();
        let task = scoped_task::spawn(
            async move {
                use rand::Rng;

                tracing::debug!("hole punching started");

                // Using RAII to log the message even when the task is aborted.
                struct Guard(Span);

                impl Drop for Guard {
                    fn drop(&mut self) {
                        tracing::debug!(parent: &self.0, "hole punching stopped");
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
                        Err(error) => tracing::warn!("hole punch failed: {:?}", error),
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
            Err(e) => {
                tracing::warn!(
                    parent: &span,
                    bind_addr = %PeerAddr::Quic(bind_addr),
                    "Failed to start listener: {:?}", e
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
            Err(err) => {
                tracing::warn!(
                    parent: &span,
                    bind_addr = %PeerAddr::Tcp(bind_addr),
                    "Failed to start listener: {:?}", err
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
            Err(err) => {
                tracing::warn!(
                    parent: &span,
                    bind_addr = %PeerAddr::Tcp(bind_addr),
                    "Failed to get listener local address: {:?}",
                    err
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
                tracing::error!("Failed to accept incoming connection: {}", error);
                break;
            }
        }
    }
}

async fn run_quic_listener(
    mut listener: quic::Acceptor,
    tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
) {
    loop {
        let result = select! {
            result = listener.accept() => result,
            _ = tx.closed() => break,
        };

        match result {
            Ok(socket) => {
                let addr = *socket.remote_address();
                tx.send((raw::Stream::Quic(socket), PeerAddr::Quic(addr)))
                    .await
                    .ok();
            }
            Err(error) => {
                tracing::error!("Failed to accept incoming connection: {}", error);
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
