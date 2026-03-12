use std::{
    io,
    net::SocketAddr,
    sync::atomic::{AtomicUsize, Ordering},
};

use ouisync::{DatagramSocket, UdpSocket};
use ouisync_macros::api;
use serde::{Deserialize, Serialize};
use slab::Slab;
use tokio::{
    select,
    sync::{Notify, RwLock},
};

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
    set: RwLock<Slab<UdpSocket>>,
    update_lock: Lock,
}

impl SocketSet {
    pub fn new() -> Self {
        Self {
            set: RwLock::new(Slab::new()),
            update_lock: Lock::new(),
        }
    }

    pub fn insert(&self, socket: UdpSocket) -> SocketHandle {
        // Note: block_write is not actually blocking, because we acquire `update_lock` which
        // immediatelly interrupts any ongoing read locks.
        let _guard = self.update_lock.acquire();
        let handle = self.set.blocking_write().insert(socket);

        SocketHandle(handle)
    }

    pub fn remove(&self, handle: SocketHandle) {
        let _guard = self.update_lock.acquire();
        self.set.blocking_write().try_remove(handle.0);
    }

    pub async fn send_to(
        &self,
        handle: SocketHandle,
        buf: &[u8],
        target: SocketAddr,
    ) -> io::Result<usize> {
        loop {
            self.update_lock.wait_until(false).await;

            let set = self.set.read().await;
            let socket = set.get(handle.0).ok_or_else(socket_not_found)?;

            select! {
                result = socket.send_to(buf, target) => break result,
                _ = self.update_lock.wait_until(true) => continue,
            }
        }
    }

    pub async fn recv_from(
        &self,
        handle: SocketHandle,
        buf: &mut [u8],
    ) -> io::Result<(usize, SocketAddr)> {
        loop {
            self.update_lock.wait_until(false).await;

            let set = self.set.read().await;
            let socket = set.get(handle.0).ok_or_else(socket_not_found)?;

            select! {
                result = socket.recv_from(buf) => break result,
                _ = self.update_lock.wait_until(true) => continue,
            }
        }
    }
}

struct Lock {
    count: AtomicUsize,
    notify: Notify,
}

impl Lock {
    fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
            notify: Notify::new(),
        }
    }

    fn acquire(&self) -> LockGuard<'_> {
        self.count.fetch_add(1, Ordering::Acquire);
        self.notify.notify_waiters();

        LockGuard(self)
    }

    fn is_acquired(&self) -> bool {
        self.count.load(Ordering::Relaxed) > 0
    }

    async fn wait_until(&self, acquired: bool) {
        loop {
            let notified = self.notify.notified();

            if self.is_acquired() == acquired {
                break;
            }

            notified.await;
        }
    }
}

struct LockGuard<'a>(&'a Lock);

impl Drop for LockGuard<'_> {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::Release);
        self.0.notify.notify_waiters();
    }
}

fn socket_not_found() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "socket not found")
}
