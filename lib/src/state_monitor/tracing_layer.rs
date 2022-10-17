use super::StateMonitor;
use std::collections::HashMap;
use std::sync::Mutex;
use tracing::span::{self, Attributes};
use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};

pub struct TracingLayer {
    inner: Mutex<TraceLayerInner>,
}

impl TracingLayer {
    pub fn new(trace_monitor: StateMonitor) -> Self {
        Self {
            inner: Mutex::new(TraceLayerInner {
                root_monitor: trace_monitor,
                spans: HashMap::new(),
            }),
        }
    }
}

struct TraceLayerInner {
    root_monitor: StateMonitor,
    spans: HashMap<SpanId, StateMonitor>,
}

type SpanId = u64;

impl<S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup>> Layer<S> for TracingLayer {
    fn on_new_span(&self, _attrs: &Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let mut inner = self.inner.lock().unwrap();
        // Unwrap should be OK since I assume the span has just been created (given the name of
        // this function).
        let span = ctx.span(id).unwrap();

        let parent_monitor = match span.parent() {
            Some(parent_span) => inner.spans.get_mut(&parent_span.id().into_u64()).unwrap(),
            None => &mut inner.root_monitor,
        };

        // There is no guarantee that the span shall have a unique name, so we need to disambiguate
        // it somehow. TODO: Maybe modify the `StateMonitor` class to include some `u64`
        // disambiguator that is not presented to the user.
        let span_monitor = parent_monitor.make_non_unique_child(span.name(), id.into_u64());

        assert!(inner.spans.insert(id.into_u64(), span_monitor).is_none());
    }

    fn on_close(&self, id: span::Id, _ctx: Context<'_, S>) {
        let mut inner = self.inner.lock().unwrap();
        inner.spans.remove(&id.into_u64());
    }
}
