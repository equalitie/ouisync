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
use tracing_subscriber::{
    layer::Context,
    registry::{LookupSpan, SpanRef},
    Layer,
};

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

// https://docs.rs/tracing-subscriber/latest/tracing_subscriber/layer/trait.Layer.html
impl<S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup>> Layer<S> for TracingLayer {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let mut inner = self.inner.lock().unwrap();
        // Unwrap should be OK since I assume the span has just been created (given the name of
        // this function).
        let span = ctx.span(id).unwrap();
        inner.on_new_span(attrs, id, span);
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut inner = self.inner.lock().unwrap();
        // Unwrap should be OK since I assume the span has just been created (given the name of
        // this function).
        let span_id = ctx.current_span().id().unwrap().into_u64();
        inner.on_event(event, span_id);
    }

    fn on_close(&self, id: span::Id, _ctx: Context<'_, S>) {
        let mut inner = self.inner.lock().unwrap();
        inner.on_close(id.into_u64());
    }

    fn on_record(&self, id: &span::Id, record: &Record<'_>, _ctx: Context<'_, S>) {
        let mut inner = self.inner.lock().unwrap();
        inner.on_record(id.into_u64(), record);
    }
}

//--------------------------------------------------------------------

type MonitoredValues = HashMap<&'static str, MonitoredValue<String>>;

struct TraceLayerInner {
    root_monitor: StateMonitor,
    spans: HashMap<SpanId, (StateMonitor, MonitoredValues)>,
}

impl TraceLayerInner {
    fn on_new_span<S>(&mut self, attrs: &Attributes<'_>, id: &span::Id, span: SpanRef<'_, S>)
    where
        S: for<'a> LookupSpan<'a>,
    {
        let parent_monitor = match span.parent() {
            Some(parent_span) => &mut self.spans.get_mut(&parent_span.id().into_u64()).unwrap().0,
            None => &mut self.root_monitor,
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

        let overwritten = self
            .spans
            .insert(id.into_u64(), (span_monitor, MonitoredValues::new()))
            .is_some();

        assert!(!overwritten);
    }

    fn on_event(&mut self, event: &Event<'_>, span_id: u64) {
        if let Some((monitor, values)) = self.spans.get_mut(&span_id) {
            event.record(&mut RecordVisitor { monitor, values });
        }
    }

    fn on_close(&mut self, span_id: u64) {
        self.spans.remove(&span_id);
    }

    fn on_record(&mut self, span_id: u64, record: &Record<'_>) {
        if let Some((monitor, values)) = self.spans.get_mut(&span_id) {
            record.record(&mut RecordVisitor { monitor, values });
        }
    }
}

type SpanId = u64;

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
