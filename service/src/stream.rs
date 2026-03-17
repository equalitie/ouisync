use std::{
    collections::{HashMap, hash_map::Entry},
    io,
};

use ouisync::{Network, PeerAddr, RecvStream, SendStream, TopicId};
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
    inner: CoopRwLock<Inner>,
}

impl StreamSet {
    pub fn new() -> Self {
        Self {
            inner: CoopRwLock::new(Inner::default()),
        }
    }

    /// Inserts new streams into the set, returning a handle to them.
    pub async fn insert(
        &self,
        network: &Network,
        addr: PeerAddr,
        topic_id: TopicId,
    ) -> io::Result<StreamHandle> {
        let mut inner = self.inner.write().await;
        let inner = &mut *inner;

        let key = StreamKey(addr, topic_id);

        match inner.index.entry(key) {
            Entry::Occupied(_) => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "stream already exists",
            )),
            Entry::Vacant(entry) => {
                let (send, recv) = network.open_stream(addr, topic_id).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotConnected,
                        format!("not connected: {}", addr),
                    )
                })?;
                let send = Mutex::new(send);
                let recv = Mutex::new(recv);

                let handle = inner.streams.insert(StreamHolder { key, send, recv });
                entry.insert(handle);

                Ok(StreamHandle(handle))
            }
        }
    }

    /// Read bytes from the recv stream corresponding to `handle` into `buf`. Returns the number of bytes
    /// actually read.
    pub async fn read(&self, handle: StreamHandle, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let (inner, wants_write) = self.inner.read().await;
            let mut stream = inner
                .streams
                .get(handle.0)
                .ok_or_else(stream_not_found)?
                .recv
                .lock()
                .await;

            select! {
                result = stream.read(buf) => break result,
                _ = wants_write => continue,
            }
        }
    }

    /// Reads the exact number of bytes to fill `buf` from the recv stream corresponding to `handle`.
    pub async fn read_exact(&self, handle: StreamHandle, buf: &mut [u8]) -> io::Result<usize> {
        let mut offset = 0;

        while offset < buf.len() {
            let n = self.read(handle, &mut buf[offset..]).await?;

            if n > 0 {
                offset += n;
            } else {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
        }

        Ok(offset)
    }

    /// Write bytes from `buf` to the send stream corresponding to `handle`. Returns the number of
    /// bytes actually written.
    pub async fn write(&self, handle: StreamHandle, buf: &[u8]) -> io::Result<usize> {
        loop {
            let (inner, wants_write) = self.inner.read().await;
            let mut stream = inner
                .streams
                .get(handle.0)
                .ok_or_else(stream_not_found)?
                .send
                .lock()
                .await;

            select! {
                result = stream.write(buf) => break result,
                _ = wants_write => continue,
            }
        }
    }

    /// Writes the entire `buf` to the send stream corresponding to `handle`.
    pub async fn write_all(&self, handle: StreamHandle, buf: &[u8]) -> io::Result<()> {
        let mut offset = 0;

        while offset < buf.len() {
            let n = self.write(handle, &buf[offset..]).await?;

            if n > 0 {
                offset += n;
            } else {
                return Err(io::ErrorKind::WriteZero.into());
            }
        }

        Ok(())
    }

    /// Close the send and recv streams corresponding to `handle`.
    pub async fn close(&self, handle: StreamHandle) -> io::Result<()> {
        let stream = {
            let mut inner = self.inner.write().await;

            let StreamHolder { key, send, .. } = inner
                .streams
                .try_remove(handle.0)
                .ok_or_else(stream_not_found)?;

            inner.index.remove(&key);

            send
        };

        stream.into_inner().shutdown().await
    }
}

#[derive(Default)]
struct Inner {
    streams: Slab<StreamHolder>,
    index: HashMap<StreamKey, usize>,
}

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
struct StreamKey(PeerAddr, TopicId);

struct StreamHolder {
    key: StreamKey,
    send: Mutex<SendStream>,
    recv: Mutex<RecvStream>,
}

fn stream_not_found() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "stream not found")
}
