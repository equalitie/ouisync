use std::{io, net::SocketAddr};

use ouisync::{DatagramSocket, UdpSocket};
use ouisync_macros::api;
use serde::{Deserialize, Serialize};
use slab::Slab;
use tokio::select;

use crate::coop_rw_lock::CoopRwLock;

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Debug)]
#[serde(transparent)]
#[api]
pub struct NetworkSocketHandle(usize);

pub(crate) struct NetworkSocketSet {
    inner: CoopRwLock<Slab<UdpSocket>>,
}

impl NetworkSocketSet {
    pub fn new() -> Self {
        Self {
            inner: CoopRwLock::new(Slab::new()),
        }
    }

    pub async fn insert(&self, socket: UdpSocket) -> NetworkSocketHandle {
        // Note this isn't actually blocking because any existing readers are immediately
        // interrupted and the writers don't block while holding the write lock.
        let handle = self.inner.write().await.insert(socket);
        NetworkSocketHandle(handle)
    }

    pub async fn remove(&self, handle: NetworkSocketHandle) {
        self.inner.write().await.try_remove(handle.0);
    }

    pub async fn send_to(
        &self,
        handle: NetworkSocketHandle,
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
        handle: NetworkSocketHandle,
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
