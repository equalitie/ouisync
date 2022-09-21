use crate::config::ConfigEntry;
use backoff::{backoff::Backoff, ExponentialBackoffBuilder};
use std::{future::Future, io, net::SocketAddr, pin::Pin, time::Duration};
use tokio::{
    net::{TcpListener, UdpSocket},
    time,
};

const BIND_RETRY_TIMEOUT: Duration = Duration::from_secs(10);

/// Bind socket to the given address. If the port is 0, will try to use the same port as the last
/// time this function was called. The port is loaded/stored in the given config entry.
pub(super) async fn bind<T: Socket>(
    mut addr: SocketAddr,
    config: ConfigEntry<u16>,
) -> io::Result<T> {
    let original_port = addr.port();

    if addr.port() == 0 {
        if let Ok(last_port) = config.get().await {
            addr.set_port(last_port);
        }
    }

    let socket = match bind_with_retries(addr).await {
        Ok(socket) => Ok(socket),
        Err(e) => {
            if original_port == 0 && original_port != addr.port() {
                addr.set_port(0);
                T::bind(addr).await
            } else {
                Err(e)
            }
        }
    }?;

    if let Ok(addr) = socket.local_addr() {
        // Ignore failures
        config.set(&addr.port()).await.ok();
    }

    Ok(socket)
}

/// Bind to the specified address, trying a couple of times with an exponential backoff strategy in
/// case the address is already in use.
///
/// This is a workaround for a situation where we bind to that address, drop the socket and then
/// attempt to bind to it again which sometimes still returns the "address in use" error for some
/// reason (perhaps the OS is taking its time cleaning the socket up, perhaps it's something in the
/// quinn library or on our side). Giving it a couple of atempts seems to solve it.
async fn bind_with_retries<T: Socket>(addr: SocketAddr) -> io::Result<T> {
    let mut backoff = ExponentialBackoffBuilder::new()
        .with_initial_interval(Duration::from_millis(50))
        .with_max_interval(Duration::from_millis(500))
        .with_max_elapsed_time(Some(BIND_RETRY_TIMEOUT))
        .build();

    loop {
        match T::bind(addr).await {
            Ok(socket) => {
                return Ok(socket);
            }
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                let duration = backoff.next_backoff().ok_or(error)?;
                tracing::debug!(
                    "Bind failed - address {:?} already in use. Trying again in {:?}",
                    addr,
                    duration
                );
                time::sleep(duration).await;
            }
            Err(error) => return Err(error),
        }
    }
}

// Internal trait to abstract over different types of network sockets.
pub(super) trait Socket: Sized {
    fn bind(addr: SocketAddr) -> DynIoFuture<Self>;
    fn local_addr(&self) -> io::Result<SocketAddr>;
}

type DynIoFuture<T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send>>;

impl Socket for TcpListener {
    fn bind(addr: SocketAddr) -> DynIoFuture<Self> {
        Box::pin(TcpListener::bind(addr))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        TcpListener::local_addr(self)
    }
}

impl Socket for UdpSocket {
    fn bind(addr: SocketAddr) -> DynIoFuture<Self> {
        Box::pin(UdpSocket::bind(addr))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        UdpSocket::local_addr(self)
    }
}
