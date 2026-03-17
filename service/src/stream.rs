use std::io;

use ouisync::{RecvStream, SendStream};
use ouisync_macros::api;
use serde::{Deserialize, Serialize};
use slab::Slab;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    select,
    sync::Mutex,
};

use crate::coop_rw_lock::CoopRwLock;

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Debug)]
#[serde(transparent)]
#[api]
pub struct StreamHandle(usize);

pub(crate) struct StreamSet {
    inner: CoopRwLock<Slab<(Mutex<SendStream>, Mutex<RecvStream>)>>,
}

impl StreamSet {
    pub fn new() -> Self {
        Self {
            inner: CoopRwLock::new(Slab::new()),
        }
    }

    /// Inserts new streams into the set, returning a handle to them.
    pub fn insert(&self, send_stream: SendStream, recv_stream: RecvStream) -> StreamHandle {
        StreamHandle(
            self.inner
                .blocking_write()
                .insert((Mutex::new(send_stream), Mutex::new(recv_stream))),
        )
    }

    /// Read bytes from the recv stream corresponding to `handle` into `buf`. Returns the number of bytes
    /// actually read.
    pub async fn read(&self, handle: StreamHandle, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let (inner, wants_write) = self.inner.read().await;
            let mut stream = inner
                .get(handle.0)
                .ok_or_else(stream_not_found)?
                .1
                .lock()
                .await;

            select! {
                result = stream.read(buf) => break result,
                _ = wants_write => continue,
            }
        }
    }

    /// Write bytes from `buf` to the send stream corresponding to `handle`. Returns the number of
    /// bytes actually written.
    pub async fn write(&self, handle: StreamHandle, buf: &[u8]) -> io::Result<usize> {
        loop {
            let (inner, wants_write) = self.inner.read().await;
            let mut stream = inner
                .get(handle.0)
                .ok_or_else(stream_not_found)?
                .0
                .lock()
                .await;

            select! {
                result = stream.write(buf) => break result,
                _ = wants_write => continue,
            }
        }
    }

    /// Close the send and recv streams corresponding to `handle`.
    pub async fn close(&self, handle: StreamHandle) -> io::Result<()> {
        self.inner
            .blocking_write()
            .try_remove(handle.0)
            .ok_or_else(stream_not_found)?
            .0
            .into_inner()
            .shutdown()
            .await
    }
}

fn stream_not_found() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "stream not found")
}
