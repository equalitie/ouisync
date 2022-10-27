use super::{MonitoredValue, StateMonitor};
use std::{
    collections::{hash_map, HashMap},
    fmt,
    sync::Mutex,
};
use tracing::{
    event::Event,
    field::{Field, Visit},
    span::{self, Attributes, Record},
};
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

type MonitoredValues = HashMap<&'static str, MonitoredValue<String>>;

struct TraceLayerInner {
    root_monitor: StateMonitor,
    spans: HashMap<SpanId, (StateMonitor, MonitoredValues)>,
}

type SpanId = u64;

// https://docs.rs/tracing-subscriber/latest/tracing_subscriber/layer/trait.Layer.html
impl<S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup>> Layer<S> for TracingLayer {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let mut inner = self.inner.lock().unwrap();
        // Unwrap should be OK since I assume the span has just been created (given the name of
        // this function).
        let span = ctx.span(id).unwrap();

        let parent_monitor = match span.parent() {
            Some(parent_span) => &mut inner.spans.get_mut(&parent_span.id().into_u64()).unwrap().0,
            None => &mut inner.root_monitor,
        };

        let mut visitor = AttrsVisitor::new();
        attrs.values().record(&mut visitor);
        let title = if visitor.is_empty() {
            span.name().to_owned()
        } else {
            format!("{}({})", span.name(), visitor)
        };

        // There is no guarantee that the span shall have a unique name, so we need to disambiguate
        // it somehow. TODO: Maybe modify the `StateMonitor` class to include some `u64`
        // disambiguator that is not presented to the user.
        let span_monitor = parent_monitor.make_non_unique_child(title, id.into_u64());

        let overwritten = inner
            .spans
            .insert(id.into_u64(), (span_monitor, MonitoredValues::new()))
            .is_some();

        assert!(!overwritten);
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut inner = self.inner.lock().unwrap();

        let span_id = match ctx.current_span().id() {
            Some(span_id) => span_id.into_u64(),
            // TODO: Is this a reachable state in this function?
            None => return,
        };

        if let Some((monitor, values)) = inner.spans.get_mut(&span_id) {
            event.record(&mut RecordVisitor { monitor, values });
        }
    }

    fn on_close(&self, id: span::Id, _ctx: Context<'_, S>) {
        let mut inner = self.inner.lock().unwrap();
        inner.spans.remove(&id.into_u64());
    }

    fn on_record(&self, id: &span::Id, record: &Record<'_>, _ctx: Context<'_, S>) {
        let mut inner = self.inner.lock().unwrap();

        if let Some((monitor, values)) = inner.spans.get_mut(&id.into_u64()) {
            record.record(&mut RecordVisitor { monitor, values });
        }
    }
}

//--------------------------------------------------------------------

struct AttrsVisitor {
    vec: Vec<String>,
}

impl AttrsVisitor {
    fn new() -> Self {
        Self { vec: Vec::new() }
    }

    fn is_empty(&self) -> bool {
        self.vec.is_empty()
    }
}

impl Visit for AttrsVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.vec.push(format!("{}={:?}", field.name(), value));
    }
}

impl fmt::Display for AttrsVisitor {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for (index, item) in self.vec.iter().enumerate() {
            write!(f, "{}{}", if index > 0 { ", " } else { "" }, item)?;
        }

        Ok(())
    }
}

//--------------------------------------------------------------------

struct RecordVisitor<'a> {
    monitor: &'a mut StateMonitor,
    values: &'a mut MonitoredValues,
}

impl<'a> RecordVisitor<'a> {
    fn set_value(&mut self, name: &'static str, value: String) {
        match self.values.entry(name) {
            hash_map::Entry::Occupied(mut entry) => {
                *entry.get_mut().get() = value;
            }
            hash_map::Entry::Vacant(entry) => {
                let value = self.monitor.make_value::<String>(name.into(), value);
                entry.insert(value);
            }
        }
    }
}

impl<'a> Visit for RecordVisitor<'a> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.set_value(field.name(), format!("{:?}", value));
    }
}

//--------------------------------------------------------------------
