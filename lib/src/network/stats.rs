use super::raw;
use pin_project_lite::pin_project;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, IoSlice},
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    task::{ready, Context, Poll},
    time::Instant,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Network traffic statistics.
#[derive(Default, Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct Stats {
    /// Total number of bytes sent.
    pub bytes_tx: u64,
    /// Total number of bytes received.
    pub bytes_rx: u64,
    /// Current send throughput in bytes per second.
    pub throughput_tx: u64,
    /// Current receive throughput in bytes per second.
    pub throughput_rx: u64,
}

#[derive(Default)]
pub(super) struct StatsTracker {
    pub bytes: Arc<ByteCounters>,
    throughput: Mutex<Throughputs>,
}

impl StatsTracker {
    pub fn read(&self) -> Stats {
        let bytes_tx = self.bytes.read_tx();
        let bytes_rx = self.bytes.read_rx();
        let now = Instant::now();

        let mut throughput = self.throughput.lock().unwrap();
        let throughput_tx = throughput.tx.sample(bytes_tx, now);
        let throughput_rx = throughput.rx.sample(bytes_rx, now);

        Stats {
            bytes_tx,
            bytes_rx,
            throughput_tx,
            throughput_rx,
        }
    }
}

#[derive(Default)]
struct Throughputs {
    tx: Throughput,
    rx: Throughput,
}

/// Counter of sent/received bytes
#[derive(Default)]
pub(super) struct ByteCounters {
    tx: AtomicU64,
    rx: AtomicU64,
}

impl ByteCounters {
    pub fn increment_tx(&self, by: u64) {
        self.tx.fetch_add(by, Ordering::Relaxed);
    }

    pub fn increment_rx(&self, by: u64) {
        self.rx.fetch_add(by, Ordering::Relaxed);
    }

    pub fn read_tx(&self) -> u64 {
        self.tx.load(Ordering::Relaxed)
    }

    pub fn read_rx(&self) -> u64 {
        self.rx.load(Ordering::Relaxed)
    }
}

/// Throughput caculator
#[derive(Default)]
pub(super) struct Throughput {
    prev: Option<ThroughputSample>,
}

impl Throughput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the current throughput (in bytes per second), given the current total amount of
    /// bytes (sent or received) and the current time.
    ///
    /// Note: For best results, call this in regular intervals (e.g., once per second).
    pub fn sample(&mut self, bytes: u64, timestamp: Instant) -> u64 {
        let throughput = if let Some(prev) = self.prev.take() {
            let millis = timestamp
                .saturating_duration_since(prev.timestamp)
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX);

            if millis == 0 {
                prev.throughput
            } else {
                bytes.saturating_sub(prev.bytes) * 1000 / millis
            }
        } else {
            0
        };

        self.prev = Some(ThroughputSample {
            timestamp,
            bytes,
            throughput,
        });

        throughput
    }
}

struct ThroughputSample {
    timestamp: Instant,
    bytes: u64,
    throughput: u64,
}

pin_project! {
    /// Wrapper for an IO object (reader or writer) that counts the transferred bytes.
    pub(super) struct Instrumented<T> {
        #[pin]
        inner: T,
        counters: Arc<ByteCounters>,
    }
}

impl<T> Instrumented<T> {
    pub fn new(inner: T, counters: Arc<ByteCounters>) -> Self {
        Self { inner, counters }
    }

    pub fn as_ref(&self) -> &T {
        &self.inner
    }

    pub fn as_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    pub fn counters(&self) -> &ByteCounters {
        &self.counters
    }
}

impl<T> AsyncRead for Instrumented<T>
where
    T: AsyncRead,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.project();

        let before = buf.filled().len();
        let result = ready!(this.inner.poll_read(cx, buf));

        if result.is_ok() {
            this.counters
                .increment_rx(buf.filled().len().saturating_sub(before) as u64);
        }

        Poll::Ready(result)
    }
}

impl<T> AsyncWrite for Instrumented<T>
where
    T: AsyncWrite,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        let this = self.project();
        let result = ready!(this.inner.poll_write(cx, buf));

        if let Ok(n) = result {
            this.counters.increment_tx(n as u64);
        }

        Poll::Ready(result)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        self.project().inner.poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        let this = self.project();
        let result = ready!(this.inner.poll_write_vectored(cx, bufs));

        if let Ok(n) = result {
            this.counters.increment_tx(n as u64);
        }

        Poll::Ready(result)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

impl Instrumented<raw::Stream> {
    pub fn into_split(
        self,
    ) -> (
        Instrumented<raw::OwnedReadHalf>,
        Instrumented<raw::OwnedWriteHalf>,
    ) {
        let (reader, writer) = self.inner.into_split();

        (
            Instrumented::new(reader, self.counters.clone()),
            Instrumented::new(writer, self.counters),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn throughput_sanity_check() {
        let mut throughput = Throughput::default();
        let start = Instant::now();

        assert_eq!(throughput.sample(1024, start), 0);
        assert_eq!(throughput.sample(1024, start + s(1)), 0);
        assert_eq!(throughput.sample(2 * 1024, start + s(2)), 1024);
        assert_eq!(throughput.sample(3 * 1024, start + s(3)), 1024);
    }

    #[test]
    fn throughput_zero_duration() {
        let mut throughput = Throughput::default();
        let start = Instant::now();

        assert_eq!(throughput.sample(1024, start), 0);
        assert_eq!(throughput.sample(1024, start), 0);
        assert_eq!(throughput.sample(2048, start), 0);

        assert_eq!(throughput.sample(2048, start + s(1)), 0);
        assert_eq!(throughput.sample(3072, start + s(1)), 0);

        assert_eq!(throughput.sample(4096, start + s(2)), 1024);
        assert_eq!(throughput.sample(5120, start + s(2)), 1024);
    }

    #[test]
    fn throughput_negative_duration() {
        let mut throughput = Throughput::default();
        let start = Instant::now();

        assert_eq!(throughput.sample(1024, start), 0);
        assert_eq!(throughput.sample(2048, start + s(1)), 1024);
        assert_eq!(throughput.sample(3072, start), 1024);
    }

    #[test]
    fn throughput_non_monotonic_bytes() {
        let mut throughput = Throughput::default();
        let start = Instant::now();

        assert_eq!(throughput.sample(1024, start), 0);
        assert_eq!(throughput.sample(2048, start + s(1)), 1024);
        assert_eq!(throughput.sample(1024, start + s(2)), 0);
    }

    fn s(value: u64) -> Duration {
        Duration::from_secs(value)
    }
}
