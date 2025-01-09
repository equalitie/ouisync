use std::io;

use ouisync_tracing_fmt::Formatter;
use tracing::{Level, Subscriber};
use tracing_subscriber::{fmt::MakeWriter, registry::LookupSpan, Layer};

pub type Callback = dyn Fn(Level, &[u8]) + Send + Sync + 'static;

/// Tracing layer that logs by calling the provided callback.
pub(super) fn layer<S>(callback: Box<Callback>) -> impl Layer<S>
where
    S: Subscriber,
    for<'a> S: LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer()
        .event_format(Formatter::default().with_level(false))
        .with_ansi(false)
        .with_writer(MakeCallbackWriter { callback })
}

struct MakeCallbackWriter {
    callback: Box<Callback>,
}

impl<'a> MakeWriter<'a> for MakeCallbackWriter {
    type Writer = CallbackWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        CallbackWriter::new(Level::DEBUG, &self.callback)
    }

    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> Self::Writer {
        CallbackWriter::new(*meta.level(), &self.callback)
    }
}

struct CallbackWriter<'a> {
    level: Level,
    callback: &'a Callback,
    buffer: Vec<u8>,
}

impl<'a> CallbackWriter<'a> {
    fn new(level: Level, callback: &'a Callback) -> Self {
        Self {
            level,
            callback,
            buffer: Vec::new(),
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
        // Trim the trailing newline
        if self.buffer.last().copied() == Some(b'\n') {
            self.buffer.pop();
        }

        // Terminate with nul byte to enable zero-cost conversion to a C-style string.
        self.buffer.push(0);

        (self.callback)(self.level, &self.buffer)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        str,
        sync::{Arc, Mutex},
    };

    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    #[test]
    fn sanity_check() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(layer(Box::new({
            let events = events.clone();
            move |level, message| {
                events
                    .lock()
                    .unwrap()
                    .push((level, str::from_utf8(message).unwrap().to_owned()))
            }
        })));

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
                    format!("foo :: {}:{}\0", file!(), base_line + 2)
                ),
                (
                    Level::DEBUG,
                    format!("bar 42 :: {}:{}\0", file!(), base_line + 3)
                ),
                (
                    Level::INFO,
                    format!("baz a=1 b=false :: {}:{}\0", file!(), base_line + 4),
                ),
                (
                    Level::WARN,
                    format!("in span :: span :: {}:{}\0", file!(), base_line + 6)
                )
            ]
        );
    }
}
