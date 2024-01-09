use metrics::{
    Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, IntoLabels, Key, KeyName, Label,
    Metadata, Recorder, SharedString, Unit,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

/// Wrapper for `Arc<dyn Recorder>` which itself implement `Recorder`
// TODO: Consider creating a PR upstread that implements `Recorder` for `Arc<impl Recorder>`.
#[derive(Clone)]
pub(crate) struct ArcRecorder(Arc<dyn Recorder + Send + Sync + 'static>);

impl ArcRecorder {
    pub fn new<R>(inner: R) -> Self
    where
        R: Recorder + Send + Sync + 'static,
    {
        Self(Arc::new(inner))
    }
}

impl Recorder for ArcRecorder {
    fn describe_counter(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.0.describe_counter(key, unit, description)
    }

    fn describe_gauge(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.0.describe_gauge(key, unit, description)
    }

    fn describe_histogram(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.0.describe_histogram(key, unit, description)
    }

    fn register_counter(&self, key: &Key, metadata: &Metadata<'_>) -> Counter {
        self.0.register_counter(key, metadata)
    }

    fn register_gauge(&self, key: &Key, metadata: &Metadata<'_>) -> Gauge {
        self.0.register_gauge(key, metadata)
    }

    fn register_histogram(&self, key: &Key, metadata: &Metadata<'_>) -> Histogram {
        self.0.register_histogram(key, metadata)
    }
}

/// Recorder which fans-out to a pair of recorders.
pub(crate) struct PairRecorder<R0, R1>(pub R0, pub R1);

impl<R0: Recorder, R1: Recorder> Recorder for PairRecorder<R0, R1> {
    fn describe_counter(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.0
            .describe_counter(key.clone(), unit, description.clone());
        self.1.describe_counter(key, unit, description);
    }

    fn describe_gauge(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.0
            .describe_gauge(key.clone(), unit, description.clone());
        self.1.describe_gauge(key, unit, description);
    }

    fn describe_histogram(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.0
            .describe_histogram(key.clone(), unit, description.clone());
        self.1.describe_histogram(key, unit, description);
    }

    fn register_counter(&self, key: &Key, metadata: &Metadata<'_>) -> Counter {
        let c0 = self.0.register_counter(key, metadata);
        let c1 = self.1.register_counter(key, metadata);

        Counter::from_arc(Arc::new(PairCounter(c0, c1)))
    }

    fn register_gauge(&self, key: &Key, metadata: &Metadata<'_>) -> Gauge {
        let g0 = self.0.register_gauge(key, metadata);
        let g1 = self.1.register_gauge(key, metadata);

        Gauge::from_arc(Arc::new(PairGauge(g0, g1)))
    }

    fn register_histogram(&self, key: &Key, metadata: &Metadata<'_>) -> Histogram {
        let h0 = self.0.register_histogram(key, metadata);
        let h1 = self.1.register_histogram(key, metadata);

        Histogram::from_arc(Arc::new(PairHistogram(h0, h1)))
    }
}

struct PairCounter(Counter, Counter);

impl CounterFn for PairCounter {
    fn increment(&self, value: u64) {
        self.0.increment(value);
        self.1.increment(value);
    }

    fn absolute(&self, value: u64) {
        self.0.absolute(value);
        self.1.absolute(value);
    }
}

struct PairGauge(Gauge, Gauge);

impl GaugeFn for PairGauge {
    fn increment(&self, value: f64) {
        self.0.increment(value);
        self.1.increment(value);
    }

    fn decrement(&self, value: f64) {
        self.0.decrement(value);
        self.1.decrement(value);
    }

    fn set(&self, value: f64) {
        self.0.set(value);
        self.1.set(value);
    }
}

struct PairHistogram(Histogram, Histogram);

impl HistogramFn for PairHistogram {
    fn record(&self, value: f64) {
        self.0.record(value);
        self.1.record(value);
    }
}

/// Adds labels to every metric key.
pub(crate) struct AddLabels<R> {
    labels: Vec<Label>,
    inner: R,
}

impl<R> AddLabels<R> {
    pub fn new(labels: Vec<Label>, inner: R) -> Self {
        Self { labels, inner }
    }

    fn add_labels(&self, key: &Key) -> Key {
        key.with_extra_labels(self.labels.clone())
    }
}

impl<R: Recorder> Recorder for AddLabels<R> {
    fn describe_counter(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.inner.describe_counter(key, unit, description)
    }

    fn describe_gauge(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.inner.describe_gauge(key, unit, description)
    }

    fn describe_histogram(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.inner.describe_histogram(key, unit, description)
    }

    fn register_counter(&self, key: &Key, metadata: &Metadata<'_>) -> Counter {
        let key = self.add_labels(key);
        self.inner.register_counter(&key, metadata)
    }

    fn register_gauge(&self, key: &Key, metadata: &Metadata<'_>) -> Gauge {
        let key = self.add_labels(key);
        self.inner.register_gauge(&key, metadata)
    }

    fn register_histogram(&self, key: &Key, metadata: &Metadata<'_>) -> Histogram {
        let key = self.add_labels(key);
        self.inner.register_histogram(&key, metadata)
    }
}

/// Reports metric values to watches (tokio::sync::watch).
pub(crate) struct WatchRecorder {
    inner: Arc<Mutex<WatchRecorderInner>>,
}

impl WatchRecorder {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(WatchRecorderInner::default())),
        }
    }

    pub fn subscriber(&self) -> MetricsSubscriber {
        MetricsSubscriber {
            inner: self.inner.clone(),
        }
    }
}

impl Recorder for WatchRecorder {
    fn describe_counter(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}

    fn describe_gauge(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}

    fn describe_histogram(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}

    fn register_counter(&self, key: &Key, _metadata: &Metadata<'_>) -> Counter {
        self.inner
            .lock()
            .unwrap()
            .counters
            .get(key.name())
            .cloned()
            .map(Counter::from_arc)
            .unwrap_or_else(Counter::noop)
    }

    fn register_gauge(&self, key: &Key, _metadata: &Metadata<'_>) -> Gauge {
        self.inner
            .lock()
            .unwrap()
            .gauges
            .get(key.name())
            .cloned()
            .map(Gauge::from_arc)
            .unwrap_or_else(Gauge::noop)
    }

    fn register_histogram(&self, key: &Key, _metadata: &Metadata<'_>) -> Histogram {
        // histograms are currently not supported
        Histogram::noop()
    }
}

#[derive(Clone)]
pub(crate) struct MetricsSubscriber {
    inner: Arc<Mutex<WatchRecorderInner>>,
}

/// Subscribe to changes to metrics. NOTE: the subscriptions must be created before the
/// corresponding metric is registered otherwise the metric will be a noop metric.
impl MetricsSubscriber {
    pub fn counter(&self, key: KeyName) -> watch::Receiver<u64> {
        self.inner
            .lock()
            .unwrap()
            .counters
            .entry(key)
            .or_insert_with(|| Arc::new(WatchCounter(watch::Sender::new(0))))
            .0
            .subscribe()
    }

    pub fn gauge(&self, key: KeyName) -> watch::Receiver<f64> {
        self.inner
            .lock()
            .unwrap()
            .gauges
            .entry(key)
            .or_insert_with(|| Arc::new(WatchGauge(watch::Sender::new(0.0))))
            .0
            .subscribe()
    }
}

#[derive(Default)]
struct WatchRecorderInner {
    counters: HashMap<KeyName, Arc<WatchCounter>>,
    gauges: HashMap<KeyName, Arc<WatchGauge>>,
}

struct WatchCounter(watch::Sender<u64>);

impl CounterFn for WatchCounter {
    fn increment(&self, value: u64) {
        self.0.send_modify(|curr| *curr += value)
    }

    fn absolute(&self, value: u64) {
        self.0.send_modify(|curr| *curr = (*curr).max(value))
    }
}

struct WatchGauge(watch::Sender<f64>);

impl GaugeFn for WatchGauge {
    fn increment(&self, value: f64) {
        self.0.send_modify(|curr| *curr += value)
    }

    fn decrement(&self, value: f64) {
        self.0.send_modify(|curr| *curr -= value)
    }

    fn set(&self, value: f64) {
        self.0.send_modify(|curr| *curr = value)
    }
}
