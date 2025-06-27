use ouisync_tracing_fmt::Formatter;
use paranoid_android::{AndroidLogMakeWriter, Buffer};
use tracing::Subscriber;
use tracing_subscriber::{fmt, registry::LookupSpan, Layer};

use super::{LogColor, LogFormat};

const TAG: &str = "ouisync";

pub(super) fn layer<S>(_format: LogFormat, _color: LogColor) -> impl Layer<S>
where
    S: Subscriber,
    for<'a> S: LookupSpan<'a>,
{
    fmt::layer()
        .event_format(Formatter::<()>::default()) // android log adds its own timestamp
        .with_ansi(false)
        .with_writer(AndroidLogMakeWriter::with_buffer(
            TAG.to_string(),
            Buffer::Main,
        ))
}
