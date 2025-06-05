use std::{
    io, mem,
    sync::{Arc, Mutex},
};

use ouisync_tracing_fmt::Formatter;
use tracing::{Level, Subscriber};
use tracing_subscriber::{fmt::MakeWriter, registry::LookupSpan, Layer};

pub type Callback = dyn Fn(Level, &mut Vec<u8>) + Send + Sync + 'static;

#[derive(Clone)]
pub struct BufferPool {
    buffers: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl BufferPool {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffers: Arc::new(Mutex::new(Vec::with_capacity(capacity))),
        }
    }

    pub fn acquire(&self) -> Vec<u8> {
        let mut buffer = self.buffers.lock().unwrap().pop().unwrap_or_default();
        buffer.clear();
        buffer
    }

    pub fn release(&self, buffer: Vec<u8>) {
        if buffer.capacity() == 0 {
            return;
        }

        // TODO: use `push_within_capacity` when stabilized (https://doc.rust-lang.org/std/vec/struct.Vec.html#method.push_within_capacity)
        let mut buffers = self.buffers.lock().unwrap();
        if buffers.len() < buffers.capacity() {
            buffers.push(buffer);
        }
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new(32)
    }
}

/// Tracing layer that logs by calling the provided callback.
pub(super) fn layer<S>(callback: Box<Callback>, pool: BufferPool) -> impl Layer<S>
where
    S: Subscriber,
    for<'a> S: LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer()
        .event_format(Formatter::default().with_level(false))
        .with_ansi(false)
        .with_writer(MakeCallbackWriter { pool, callback })
}

struct MakeCallbackWriter {
    callback: Box<Callback>,
    pool: BufferPool,
}

impl<'a> MakeWriter<'a> for MakeCallbackWriter {
    type Writer = CallbackWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        CallbackWriter::new(self, Level::DEBUG)
    }

    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> Self::Writer {
        CallbackWriter::new(self, *meta.level())
    }
}

struct CallbackWriter<'a> {
    make: &'a MakeCallbackWriter,
    level: Level,
    buffer: Vec<u8>,
}

impl<'a> CallbackWriter<'a> {
    fn new(make: &'a MakeCallbackWriter, level: Level) -> Self {
        let buffer = make.pool.acquire();

        Self {
            make,
            level,
            buffer,
        }
    }
}

impl io::Write for CallbackWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.buffer.flush()
    }
}

impl Drop for CallbackWriter<'_> {
    fn drop(&mut self) {
        // Remove trailing newlines
        while self
            .buffer
            .last()
            .map(|c| c.is_ascii_whitespace())
            .unwrap_or(false)
        {
            self.buffer.pop();
        }

        (self.make.callback)(self.level, &mut self.buffer);
        self.make.pool.release(mem::take(&mut self.buffer));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    #[test]
    fn sanity_check() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(layer(
            Box::new({
                let events = events.clone();
                move |level, message| {
                    events
                        .lock()
                        .unwrap()
                        .push((level, String::from_utf8(mem::take(message)).unwrap()));
                }
            }),
            BufferPool::default(),
        ));

        let base_line = line!();
        tracing::subscriber::with_default(subscriber, || {
            tracing::trace!("foo");
            tracing::debug!("bar {}", 42);
            tracing::info!(a = 1, b = false, "baz");
            tracing::info_span!("span").in_scope(|| {
                tracing::warn!("in span");
            });
        });

        assert_eq!(
            *events.lock().unwrap(),
            [
                (
                    Level::TRACE,
                    format!("foo :: {}:{}", file!(), base_line + 2)
                ),
                (
                    Level::DEBUG,
                    format!("bar 42 :: {}:{}", file!(), base_line + 3)
                ),
                (
                    Level::INFO,
                    format!("baz a=1 b=false :: {}:{}", file!(), base_line + 4),
                ),
                (
                    Level::WARN,
                    format!("in span :: span :: {}:{}", file!(), base_line + 6)
                )
            ]
        );
    }
}
