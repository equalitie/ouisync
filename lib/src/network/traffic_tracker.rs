use pin_project_lite::pin_project;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, IoSlice},
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{ready, Context, Poll},
    time::{Duration, SystemTime},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Tracks the amount of data exchanged with other peers.
#[derive(Default, Clone)]
pub(super) struct TrafficTracker {
    state: Arc<State>,
}

impl TrafficTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_send(&self, bytes: u64) {
        self.state.send.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn record_recv(&self, bytes: u64) {
        self.state.recv.fetch_add(bytes, Ordering::Relaxed);
        self.state
            .recv_at
            .store(SystemTime::now(), Ordering::Relaxed);
    }

    pub fn get(&self) -> TrafficStats {
        self.state.get()
    }
}

/// Network traffic statistics.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct TrafficStats {
    /// Total number of bytes sent
    pub send: u64,
    /// Total number of bytes received
    pub recv: u64,
    /// Time of the last receive. If nothing has been received yet, equals to `UNIX_EPOCH`.
    #[serde(with = "as_millis_since_epoch")]
    pub recv_at: SystemTime,
}

mod as_millis_since_epoch {
    use serde::{ser::Error as _, Deserialize, Deserializer, Serializer};
    use std::time::SystemTime;

    use crate::time;

    pub(super) fn serialize<S>(time: &SystemTime, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_u64(time::to_millis_since_epoch(*time).map_err(S::Error::custom)?)
    }

    pub(super) fn deserialize<'de, D>(d: D) -> Result<SystemTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(time::from_millis_since_epoch(u64::deserialize(d)?))
    }
}

impl Default for TrafficStats {
    fn default() -> Self {
        Self {
            send: 0,
            recv: 0,
            recv_at: SystemTime::UNIX_EPOCH,
        }
    }
}

#[derive(Default)]
struct State {
    send: AtomicU64,
    recv: AtomicU64,
    recv_at: AtomicSystemTime,
}

impl State {
    fn get(&self) -> TrafficStats {
        TrafficStats {
            send: self.send.load(Ordering::Relaxed),
            recv: self.recv.load(Ordering::Relaxed),
            recv_at: self.recv_at.load(Ordering::Relaxed),
        }
    }
}

/// Atomic version of `SystemTime`. Has only millisecond precision. Supports times from Jan 1st,
/// 1970, 00:00:00 to Apr 3, 584556019, 14:25:52 (from UNIX_EPOCH to UNIX_EPOCH + u64::MAX ms). Values
/// outside of this range are silently saturated.
#[derive(Default)]
struct AtomicSystemTime(AtomicU64);

impl AtomicSystemTime {
    fn store(&self, time: SystemTime, ordering: Ordering) {
        self.0.store(
            time.duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            ordering,
        )
    }

    fn load(&self, ordering: Ordering) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_millis(self.0.load(ordering))
    }
}

pin_project! {
    /// Adds traffic tracking to an IO object (reader or writer).
    pub(super) struct TrackingWrapper<T> {
        #[pin]
        inner: T,
        tracker: TrafficTracker,
    }
}

impl<T> TrackingWrapper<T> {
    pub fn new(inner: T, tracker: TrafficTracker) -> Self {
        Self { inner, tracker }
    }
}

impl<T> AsyncRead for TrackingWrapper<T>
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
            this.tracker
                .record_recv(buf.filled().len().saturating_sub(before) as u64);
        }

        Poll::Ready(result)
    }
}

impl<T> AsyncWrite for TrackingWrapper<T>
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
            this.tracker.record_send(n as u64);
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
            this.tracker.record_send(n as u64);
        }

        Poll::Ready(result)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}
