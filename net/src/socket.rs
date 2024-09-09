use socket2::Socket;
use std::{io, net::SocketAddr};

/// Binds socket to the given address. If the port is already taken, binds to a random available
/// port which can be retrieved by calling `local_addr` on the returned socket.
pub(crate) fn bind_with_fallback(socket: &Socket, mut addr: SocketAddr) -> io::Result<()> {
    match socket.bind(&addr.into()) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Try again with random port, unless we already used random port initially.
            if addr.port() != 0 {
                addr.set_port(0);
                socket.bind(&addr.into())
            } else {
                Err(e)
            }
        }
    }
}

/// Options for the underlying network socket.
#[derive(Clone, Copy, Default, Debug)]
pub struct SocketOptions {
    pub(crate) reuse_addr: bool,
}

impl SocketOptions {
    /// Enables the `SO_REUSEADDR` option on the socket.
    pub fn with_reuse_addr(self) -> Self {
        Self { reuse_addr: true }
    }
}
