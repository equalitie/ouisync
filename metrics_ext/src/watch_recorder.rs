use metrics::{
    Counter, CounterFn, Gauge, GaugeFn, Histogram, Key, KeyName, Metadata, Recorder, SharedString,
    Unit,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

/// Reports metric values to watches (tokio::sync::watch).
#[derive(Default)]
pub struct WatchRecorder {
    inner: Arc<Mutex<Inner>>,
}

impl WatchRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscriber(&self) -> WatchRecorderSubscriber {
        WatchRecorderSubscriber {
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

    fn register_histogram(&self, _key: &Key, _metadata: &Metadata<'_>) -> Histogram {
        // histograms are currently not supported
        Histogram::noop()
    }
}

#[derive(Clone)]
pub struct WatchRecorderSubscriber {
    inner: Arc<Mutex<Inner>>,
}

/// Subscribe to changes to metrics. NOTE: the subscriptions must be created before the
/// corresponding metric is registered otherwise the metric will be a noop metric.
impl WatchRecorderSubscriber {
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
struct Inner {
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
