use std::{io, net::SocketAddr};

use ouisync::{DatagramSocket, UdpSocket};
use ouisync_macros::api;
use serde::{Deserialize, Serialize};
use slab::Slab;
use tokio::select;

use crate::coop_rw_lock::CoopRwLock;

// The api parser doesn't support raw tuples so we need to use actual named structs.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
#[api]
pub struct SocketRecv {
    pub data: Vec<u8>,
    pub addr: SocketAddr,
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Debug)]
#[serde(transparent)]
#[api]
pub struct SocketHandle(usize);

pub(crate) struct SocketSet {
    inner: CoopRwLock<Slab<UdpSocket>>,
}

impl SocketSet {
    pub fn new() -> Self {
        Self {
            inner: CoopRwLock::new(Slab::new()),
        }
    }

    pub async fn insert(&self, socket: UdpSocket) -> SocketHandle {
        // Note this isn't actually blocking because any existing readers are immediately
        // interrupted and the writers don't block while holding the write lock.
        let handle = self.inner.write().await.insert(socket);
        SocketHandle(handle)
    }

    pub async fn remove(&self, handle: SocketHandle) {
        self.inner.write().await.try_remove(handle.0);
    }

    pub async fn send_to(
        &self,
        handle: SocketHandle,
        buf: &[u8],
        target: SocketAddr,
    ) -> io::Result<usize> {
        loop {
            let (inner, wants_write) = self.inner.read().await;
            let socket = inner.get(handle.0).ok_or_else(socket_not_found)?;

            select! {
                result = socket.send_to(buf, target) => break result,
                _ = wants_write => continue,
            }
        }
    }

    pub async fn recv_from(
        &self,
        handle: SocketHandle,
        buf: &mut [u8],
    ) -> io::Result<(usize, SocketAddr)> {
        loop {
            let (inner, wants_write) = self.inner.read().await;
            let socket = inner.get(handle.0).ok_or_else(socket_not_found)?;

            select! {
                result = socket.recv_from(buf) => break result,
                _ = wants_write => continue,
            }
        }
    }
}

fn socket_not_found() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "socket not found")
}
