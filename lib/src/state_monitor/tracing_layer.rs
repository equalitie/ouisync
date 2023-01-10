use super::{MonitoredValue, StateMonitor};
use crate::collections::{hash_map, HashMap};
use std::{
    fmt,
    sync::{Arc, Mutex},
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

#[derive(Clone, Default)]
pub struct TracingLayer {
    inner: Arc<Mutex<Option<TraceLayerInner>>>,
}

impl TracingLayer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_monitor(&self, trace_monitor: Option<StateMonitor>) {
        let mut inner = self.inner.lock().unwrap();

        match trace_monitor {
            Some(trace_monitor) => {
                *inner = Some(TraceLayerInner {
                    root_span: Span {
                        monitor: trace_monitor,
                        values: MonitoredValues::default(),
                    },
                    root_message: None,
                    spans: HashMap::default(),
                })
            }
            None => *inner = None,
        }
    }
}

// https://docs.rs/tracing-subscriber/latest/tracing_subscriber/layer/trait.Layer.html
impl<S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup>> Layer<S> for TracingLayer {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let mut guard = self.inner.lock().unwrap();
        let inner = match guard.as_mut() {
            Some(inner) => inner,
            None => panic!("Tracing started prior to setting a monitor (on_new_span)"),
        };
        // Unwrap should be OK since I assume the span has just been created (given the name of
        // this function).
        let span = ctx.span(id).unwrap();
        inner.on_new_span(attrs, id, span);
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut guard = self.inner.lock().unwrap();
        let inner = match guard.as_mut() {
            Some(inner) => inner,
            None => panic!("Tracing started prior to setting a monitor (on_event)"),
        };
        let span_id = ctx.current_span().id().map(|id| id.into_u64());
        inner.on_event(event, span_id);
    }

    fn on_close(&self, id: span::Id, _ctx: Context<'_, S>) {
        let mut guard = self.inner.lock().unwrap();
        let inner = match guard.as_mut() {
            Some(inner) => inner,
            None => panic!("Tracing started prior to setting a monitor (on_close)"),
        };
        inner.on_close(id.into_u64());
    }

    fn on_record(&self, id: &span::Id, record: &Record<'_>, _ctx: Context<'_, S>) {
        let mut guard = self.inner.lock().unwrap();
        let inner = match guard.as_mut() {
            Some(inner) => inner,
            None => panic!("Tracing started prior to setting a monitor (on_record)"),
        };
        inner.on_record(id.into_u64(), record);
    }
}

//--------------------------------------------------------------------

type MonitoredValues = HashMap<&'static str, MonitoredValue<String>>;

struct Span {
    monitor: StateMonitor,
    values: MonitoredValues,
}

struct Message {
    _monitor: StateMonitor,
    _values: MonitoredValues,
}

struct TraceLayerInner {
    root_span: Span,
    root_message: Option<Message>,
    spans: HashMap<SpanId, (Span, Option<Message>)>,
}

impl TraceLayerInner {
    fn on_new_span<S>(&mut self, attrs: &Attributes<'_>, id: &span::Id, span: SpanRef<'_, S>)
    where
        S: for<'a> LookupSpan<'a>,
    {
        let parent_monitor = match span.parent() {
            Some(parent_span) => {
                &mut self
                    .spans
                    .get_mut(&parent_span.id().into_u64())
                    .unwrap()
                    .0
                    .monitor
            }
            None => &mut self.root_span.monitor,
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
            .insert(
                id.into_u64(),
                (
                    Span {
                        monitor: span_monitor,
                        values: MonitoredValues::default(),
                    },
                    None,
                ),
            )
            .is_some();

        assert!(!overwritten);
    }

    fn on_event(&mut self, event: &Event<'_>, current_span_id: Option<u64>) {
        // `event.parent()` is the span that is explicitly passed to a tracing macro (if any).
        let span_id = event
            .parent()
            .map(|span_id| span_id.into_u64())
            .or(current_span_id);

        // It sometimes happens that we get an event that doesn't have a span. We shove it all into
        // the root monitor, although this may not be 100% correct.
        let (span, message) = match span_id {
            Some(span_id) => match self.spans.get_mut(&span_id) {
                Some((span, message)) => (span, message),
                None => (&mut self.root_span, &mut self.root_message),
            },
            None => (&mut self.root_span, &mut self.root_message),
        };

        let msg = message_string(event);

        if let Some(msg) = msg {
            // Admittedly a bit hacky: if the event is a message event, then instead of showing
            // each field as `MonitoredValue` we create a new `StateMonitor` with the message as
            // its name and if there are any other fields we show them as `MonitoredValues` of this
            // newly created `StateMonitor`.
            message.take();
            let monitor = span.monitor.make_child(format!("MSG: {}", msg));
            let mut values = MonitoredValues::default();
            for_each_field(event, |field, value| {
                if field.name() != "message" {
                    let value = format!("{:?}", value);
                    match values.entry(field.name()) {
                        hash_map::Entry::Occupied(entry) => *(entry.get().get()) = value,
                        hash_map::Entry::Vacant(entry) => {
                            let value = monitor.make_value::<String>(field.name().into(), value);
                            entry.insert(value);
                        }
                    }
                }
            });
            *message = Some(Message {
                _monitor: monitor,
                _values: values,
            });
        } else {
            event.record(&mut RecordVisitor { span });
        }
    }

    fn on_close(&mut self, span_id: u64) {
        self.spans.remove(&span_id);
    }

    fn on_record(&mut self, span_id: u64, record: &Record<'_>) {
        let span = match self.spans.get_mut(&span_id) {
            Some((span, _message)) => span,
            None => return,
        };

        record.record(&mut RecordVisitor { span });
    }
}

type SpanId = u64;

//--------------------------------------------------------------------
struct ForEachField<F> {
    func: F,
}

impl<F> ForEachField<F> {
    fn new(func: F) -> Self {
        Self { func }
    }
}

impl<F: FnMut(&Field, &dyn fmt::Debug)> Visit for ForEachField<F> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        (self.func)(field, value);
    }
}

fn for_each_field<F>(event: &Event<'_>, func: F)
where
    F: FnMut(&Field, &dyn fmt::Debug),
{
    let mut visitor = ForEachField::new(func);
    event.record(&mut visitor);
}

fn message_string(event: &Event<'_>) -> Option<String> {
    let mut ret = None;
    for_each_field(event, |field, value| {
        if field.name() == "message" {
            ret = Some(format!("{:?}", value));
        }
    });
    ret
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
    span: &'a mut Span,
}

impl<'a> RecordVisitor<'a> {
    fn set_value(&mut self, name: &'static str, value: String) {
        match self.span.values.entry(name) {
            hash_map::Entry::Occupied(mut entry) => {
                *entry.get_mut().get() = value;
            }
            hash_map::Entry::Vacant(entry) => {
                let value = self.span.monitor.make_value::<String>(name.into(), value);
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
