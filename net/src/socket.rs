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

/// Binds socket to the given address according to the given options: If `options.strict_port` is
/// set, fails if the port is already taken. Otherwise falls back to a random port (see
/// [bind_with_fallback]).
pub(crate) fn bind(socket: &Socket, addr: SocketAddr, options: SocketOptions) -> io::Result<()> {
    if options.strict_port {
        socket.bind(&addr.into())
    } else {
        bind_with_fallback(socket, addr)
    }
}

/// Options for the underlying network socket.
#[derive(Clone, Copy, Default, Debug)]
pub struct SocketOptions {
    pub(crate) reuse_addr: bool,
    pub(crate) strict_port: bool,
}

impl SocketOptions {
    /// Enables the `SO_REUSEADDR` option on the socket.
    pub fn with_reuse_addr(self) -> Self {
        Self {
            reuse_addr: true,
            ..self
        }
    }

    /// Fail if the requested port is already taken instead of falling back to a random port.
    pub fn with_strict_port(self) -> Self {
        Self {
            strict_port: true,
            ..self
        }
    }
}
