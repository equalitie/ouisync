use crate::{MonitoredValue, StateMonitor};
use metrics::{
    Counter, CounterFn, Gauge, GaugeFn, GaugeValue, Histogram, HistogramFn, Key, KeyName, Metadata,
    Recorder, SharedString, Unit,
};
use metrics_util::Summary;
use std::{
    borrow::Cow,
    collections::HashMap,
    fmt,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::{sync::mpsc, task};

/// Adapter for StateMonitor that allows it to be used as a
/// [metrics](https://crates.io/crates/metrics)
/// [`Recorder`](https://docs.rs/metrics/latest/metrics/trait.Recorder.html).
#[derive(Clone)]
pub struct MetricsRecorder {
    tx: mpsc::UnboundedSender<Op>,
    next_handle: Arc<AtomicUsize>,
}

impl MetricsRecorder {
    pub fn new(parent_node: StateMonitor) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        task::spawn(run(parent_node, rx));

        Self {
            tx,
            next_handle: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn next_handle(&self) -> Handle {
        self.next_handle.fetch_add(1, Ordering::Relaxed)
    }
}

impl Recorder for MetricsRecorder {
    fn describe_counter(&self, key: KeyName, unit: Option<Unit>, _description: SharedString) {
        let Some(unit) = unit else {
            return;
        };

        self.tx.send(Op::Describe(key, unit)).ok();
    }

    fn describe_gauge(&self, key: KeyName, unit: Option<Unit>, _description: SharedString) {
        let Some(unit) = unit else {
            return;
        };

        self.tx.send(Op::Describe(key, unit)).ok();
    }

    fn describe_histogram(&self, key: KeyName, unit: Option<Unit>, _description: SharedString) {
        let Some(unit) = unit else {
            return;
        };

        self.tx.send(Op::Describe(key, unit)).ok();
    }

    fn register_counter(&self, key: &Key, _metadata: &Metadata<'_>) -> Counter {
        let handle = self.next_handle();
        self.tx.send(Op::CreateCounter(handle, key.clone())).ok();

        Counter::from_arc(Arc::new(CounterInner(handle, self.tx.clone())))
    }

    fn register_gauge(&self, key: &Key, _metadata: &Metadata<'_>) -> Gauge {
        let handle = self.next_handle();
        self.tx.send(Op::CreateGauge(handle, key.clone())).ok();

        Gauge::from_arc(Arc::new(GaugeInner(handle, self.tx.clone())))
    }

    fn register_histogram(&self, key: &Key, _metadata: &Metadata<'_>) -> Histogram {
        let handle = self.next_handle();
        self.tx.send(Op::CreateHistogram(handle, key.clone())).ok();

        Histogram::from_arc(Arc::new(HistogramInner(handle, self.tx.clone())))
    }
}

async fn run(parent_node: StateMonitor, mut rx: mpsc::UnboundedReceiver<Op>) {
    // False positive due to interior mutability which doesn't affect hashing in `Key`.
    #![allow(clippy::mutable_key_type)]

    let mut descriptions = HashMap::new();

    let mut counters = HashMap::new();
    let mut gauges = HashMap::new();
    let mut histograms = HashMap::new();

    while let Some(op) = rx.recv().await {
        match op {
            Op::Describe(key, unit) => {
                descriptions.insert(key, unit);
            }
            Op::CreateCounter(handle, key) => {
                counters.entry(handle).or_insert_with(|| {
                    let unit = descriptions.get(key.name()).copied();
                    make_value_recursive(&parent_node, key.name(), Formatted(0u64, unit))
                });
            }
            Op::DeleteCounter(handle) => {
                counters.remove(&handle);
            }
            Op::RecordCounter(handle, new_value) => {
                let Some(counter) = counters.get_mut(&handle) else {
                    continue;
                };

                let old_value = &mut **counter.get();

                match new_value {
                    CounterValue::Increment(new_value) => *old_value += new_value,
                    CounterValue::Absolute(new_value) => *old_value = (*old_value).max(new_value),
                }
            }
            Op::CreateGauge(handle, key) => {
                gauges.entry(handle).or_insert_with(|| {
                    let unit = descriptions.get(key.name()).copied();
                    make_value_recursive(&parent_node, key.name(), Formatted(0.0, unit))
                });
            }
            Op::DeleteGauge(handle) => {
                gauges.remove(&handle);
            }
            Op::RecordGauge(handle, new_value) => {
                let Some(gauge) = gauges.get_mut(&handle) else {
                    continue;
                };

                let old_value = &mut **gauge.get();

                match new_value {
                    GaugeValue::Absolute(new_value) => *old_value = new_value,
                    GaugeValue::Increment(new_value) => *old_value += new_value,
                    GaugeValue::Decrement(new_value) => *old_value -= new_value,
                }
            }
            Op::CreateHistogram(handle, key) => {
                histograms.entry(handle).or_insert_with(|| {
                    let unit = descriptions.get(key.name()).copied();
                    HistogramWrapper::new(
                        make_node_recursive(&parent_node, key.name()).into_owned(),
                        unit,
                    )
                });
            }
            Op::DeleteHistogram(handle) => {
                histograms.remove(&handle);
            }
            Op::RecordHistogram(handle, new_value) => {
                let Some(histogram) = histograms.get_mut(&handle) else {
                    continue;
                };

                histogram.record(new_value);
            }
        }
    }
}

fn make_node_recursive<'a>(root: &'a StateMonitor, path: &'_ str) -> Cow<'a, StateMonitor> {
    path.split('/')
        .fold(Cow::Borrowed(root), |parent, component| {
            if component.trim().is_empty() {
                parent
            } else {
                Cow::Owned(parent.make_child(component))
            }
        })
}

fn make_value_recursive<T>(root: &StateMonitor, path: &str, value: T) -> MonitoredValue<T>
where
    T: fmt::Debug + Send + 'static,
{
    let (parent_path, name) = path.rsplit_once('/').unwrap_or(("", path));
    make_node_recursive(root, parent_path).make_value(name, value)
}

type Handle = usize;

enum Op {
    Describe(KeyName, Unit),
    CreateCounter(Handle, Key),
    DeleteCounter(Handle),
    RecordCounter(Handle, CounterValue),
    CreateGauge(Handle, Key),
    DeleteGauge(Handle),
    RecordGauge(usize, GaugeValue),
    CreateHistogram(Handle, Key),
    DeleteHistogram(Handle),
    RecordHistogram(Handle, f64),
}

enum CounterValue {
    Absolute(u64),
    Increment(u64),
}

struct CounterInner(Handle, mpsc::UnboundedSender<Op>);

impl CounterFn for CounterInner {
    fn increment(&self, value: u64) {
        self.1
            .send(Op::RecordCounter(self.0, CounterValue::Increment(value)))
            .ok();
    }

    fn absolute(&self, value: u64) {
        self.1
            .send(Op::RecordCounter(self.0, CounterValue::Absolute(value)))
            .ok();
    }
}

impl Drop for CounterInner {
    fn drop(&mut self) {
        self.1.send(Op::DeleteCounter(self.0)).ok();
    }
}

struct GaugeInner(Handle, mpsc::UnboundedSender<Op>);

impl GaugeFn for GaugeInner {
    fn increment(&self, value: f64) {
        self.1
            .send(Op::RecordGauge(self.0, GaugeValue::Increment(value)))
            .ok();
    }

    fn decrement(&self, value: f64) {
        self.1
            .send(Op::RecordGauge(self.0, GaugeValue::Decrement(value)))
            .ok();
    }

    fn set(&self, value: f64) {
        self.1
            .send(Op::RecordGauge(self.0, GaugeValue::Absolute(value)))
            .ok();
    }
}

impl Drop for GaugeInner {
    fn drop(&mut self) {
        self.1.send(Op::DeleteGauge(self.0)).ok();
    }
}

struct HistogramInner(Handle, mpsc::UnboundedSender<Op>);

impl HistogramFn for HistogramInner {
    fn record(&self, value: f64) {
        self.1.send(Op::RecordHistogram(self.0, value)).ok();
    }
}

impl Drop for HistogramInner {
    fn drop(&mut self) {
        self.1.send(Op::DeleteHistogram(self.0)).ok();
    }
}

struct HistogramWrapper {
    summary: Summary,
    count: MonitoredValue<u64>,
    min: MonitoredValue<Formatted<f64>>,
    max: MonitoredValue<Formatted<f64>>,
    p50: MonitoredValue<Formatted<f64>>,
    p90: MonitoredValue<Formatted<f64>>,
    p99: MonitoredValue<Formatted<f64>>,
    p999: MonitoredValue<Formatted<f64>>,
}

impl HistogramWrapper {
    fn new(node: StateMonitor, unit: Option<Unit>) -> Self {
        Self {
            summary: Summary::with_defaults(),
            count: node.make_value("count", 0),
            min: node.make_value("min", Formatted(0.0, unit)),
            max: node.make_value("max", Formatted(0.0, unit)),
            p50: node.make_value("50%", Formatted(0.0, unit)),
            p90: node.make_value("90%", Formatted(0.0, unit)),
            p99: node.make_value("99%", Formatted(0.0, unit)),
            p999: node.make_value("99.9%", Formatted(0.0, unit)),
        }
    }

    fn record(&mut self, value: f64) {
        self.summary.add(value);

        *self.count.get() = self.summary.count() as u64;
        **self.min.get() = self.summary.min();
        **self.max.get() = self.summary.max();

        **self.p50.get() = self.summary.quantile(0.5).unwrap_or(0.0);
        **self.p90.get() = self.summary.quantile(0.9).unwrap_or(0.0);
        **self.p99.get() = self.summary.quantile(0.99).unwrap_or(0.0);
        **self.p999.get() = self.summary.quantile(0.999).unwrap_or(0.0);
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Formatted<T>(pub T, pub Option<Unit>);

impl<T> Deref for Formatted<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Formatted<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> fmt::Debug for Formatted<T>
where
    T: Human,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Human::fmt(&self.0, f)?;

        match self.1 {
            Some(Unit::Count) | None => Ok(()),
            Some(Unit::Percent) => write!(f, "%"),
            Some(Unit::Seconds) => write!(f, " s"),
            Some(Unit::Milliseconds) => write!(f, " ms"),
            Some(Unit::Microseconds) => write!(f, " μs"),
            Some(Unit::Nanoseconds) => write!(f, " ns"),
            Some(Unit::Bytes) => write!(f, " B"),
            Some(Unit::Kibibytes) => write!(f, " KiB"),
            Some(Unit::Mebibytes) => write!(f, " MiB"),
            Some(Unit::Gigibytes) => write!(f, " GiB"),
            Some(Unit::Tebibytes) => write!(f, " TiB"),
            Some(Unit::BitsPerSecond) => write!(f, " bit/s"),
            Some(Unit::KilobitsPerSecond) => write!(f, " Kbit/s"),
            Some(Unit::MegabitsPerSecond) => write!(f, " Mbit/s"),
            Some(Unit::GigabitsPerSecond) => write!(f, " Gbit/s"),
            Some(Unit::TerabitsPerSecond) => write!(f, " Tbit/s"),
            Some(Unit::CountPerSecond) => write!(f, "/s"),
        }
    }
}

/// Formats numbers oprimized for human consumption.
trait Human {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result;
}

impl Human for u64 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self)
    }
}

impl Human for f64 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let (sign, abs) = if *self < 0.0 {
            ("-", -*self)
        } else {
            ("", *self)
        };

        if abs < 0.0001 {
            write!(f, "0")
        } else if abs < 0.1 {
            write!(f, "{}{:.4}", sign, abs)
        } else if abs < 1.0 {
            write!(f, "{}{:.3}", sign, abs)
        } else if abs < 10.0 {
            write!(f, "{}{:.2}", sign, abs)
        } else if abs < 100.0 {
            write!(f, "{}{:.1}", sign, abs)
        } else {
            write!(f, "{}{:.0}", sign, abs)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MonitorId, StateMonitor, ValueError};
    use metrics::Level;

    #[tokio::test]
    async fn counter() {
        let root = StateMonitor::make_root();
        let recorder = MetricsRecorder::new(root.clone());

        let mut rx = root.subscribe();

        let counter = recorder.register_counter(
            &Key::from_name("test_counter"),
            &Metadata::new(module_path!(), Level::INFO, None),
        );

        rx.changed().await.unwrap();
        assert_eq!(root.get_value("test_counter"), Ok(Formatted(0u64, None)));

        counter.increment(1);

        rx.changed().await.unwrap();
        assert_eq!(root.get_value("test_counter"), Ok(Formatted(1u64, None)));
    }

    #[tokio::test]
    async fn gauge() {
        let root = StateMonitor::make_root();
        let recorder = MetricsRecorder::new(root.clone());

        let mut rx = root.subscribe();

        let gauge = recorder.register_gauge(
            &Key::from_name("test_gauge"),
            &Metadata::new(module_path!(), Level::INFO, None),
        );

        rx.changed().await.unwrap();
        assert_eq!(root.get_value("test_gauge"), Ok(Formatted(0.0, None)));

        gauge.increment(2.0);
        rx.changed().await.unwrap();
        assert_eq!(root.get_value("test_gauge"), Ok(Formatted(2.0, None)));

        gauge.decrement(1.0);
        rx.changed().await.unwrap();
        assert_eq!(root.get_value("test_gauge"), Ok(Formatted(1.0, None)));

        gauge.set(4.0);
        rx.changed().await.unwrap();
        assert_eq!(root.get_value("test_gauge"), Ok(Formatted(4.0, None)));
    }

    #[tokio::test]
    async fn histogram() {
        let root = StateMonitor::make_root();
        let recorder = MetricsRecorder::new(root.clone());

        let mut rx = root.subscribe();

        let histogram = recorder.register_histogram(
            &Key::from_name("test_histogram"),
            &Metadata::new(module_path!(), Level::INFO, None),
        );

        rx.changed().await.unwrap();

        let parent = root
            .locate([MonitorId::new("test_histogram".to_string(), 0)])
            .unwrap();

        assert_eq!(parent.get_value("count"), Ok(0u64));
        assert_eq!(parent.get_value("min"), Ok(Formatted(0.0, None)));
        assert_eq!(parent.get_value("max"), Ok(Formatted(0.0, None)));

        histogram.record(1.0);
        rx.changed().await.unwrap();

        assert_eq!(parent.get_value("count"), Ok(1u64));
        assert_eq!(parent.get_value("min"), Ok(Formatted(1.0, None)));
        assert_eq!(parent.get_value("max"), Ok(Formatted(1.0, None)));

        histogram.record(2.0);
        rx.changed().await.unwrap();

        assert_eq!(parent.get_value("count"), Ok(2u64));
        assert_eq!(parent.get_value("min"), Ok(Formatted(1.0, None)));
        assert_eq!(parent.get_value("max"), Ok(Formatted(2.0, None)));
    }

    #[tokio::test]
    async fn description() {
        let root = StateMonitor::make_root();
        let recorder = MetricsRecorder::new(root.clone());

        let mut rx = root.subscribe();

        let name = KeyName::from("test_counter");

        recorder.describe_counter(name.clone(), Some(Unit::Seconds), "".into());
        let _counter = recorder.register_counter(
            &Key::from_name(name),
            &Metadata::new(module_path!(), Level::INFO, None),
        );

        rx.changed().await.unwrap();
        assert_eq!(
            root.get_value("test_counter"),
            Ok(Formatted(0u64, Some(Unit::Seconds)))
        );
    }

    #[tokio::test]
    async fn unregister() {
        let root = StateMonitor::make_root();
        let recorder = MetricsRecorder::new(root.clone());

        let mut rx = root.subscribe();

        let counter = recorder.register_counter(
            &Key::from_name("test_counter"),
            &Metadata::new(module_path!(), Level::INFO, None),
        );

        rx.changed().await.unwrap();
        assert_eq!(
            root.get_value::<Formatted<u64>>("test_counter"),
            Ok(Formatted(0, None))
        );

        drop(counter);

        rx.changed().await.unwrap();
        assert_eq!(
            root.get_value::<Formatted<u64>>("test_counter"),
            Err(ValueError::NotFound)
        );
    }

    #[tokio::test]
    async fn hierarchy() {
        let root = StateMonitor::make_root();
        let recorder = MetricsRecorder::new(root.clone());

        let mut rx = root.subscribe();

        let _counter = recorder.register_counter(
            &Key::from_name("foo/bar/baz"),
            &Metadata::new(module_path!(), Level::INFO, None),
        );

        rx.changed().await.unwrap();

        let parent = root
            .locate([
                MonitorId::new("foo".to_string(), 0),
                MonitorId::new("bar".to_string(), 0),
            ])
            .unwrap();
        assert_eq!(parent.get_value("baz"), Ok(Formatted(0u64, None)));

        let _histogram = recorder.register_histogram(
            &Key::from_name("foo/bar/qux"),
            &Metadata::new(module_path!(), Level::INFO, None),
        );

        rx.changed().await.unwrap();

        let parent = root
            .locate([
                MonitorId::new("foo".to_string(), 0),
                MonitorId::new("bar".to_string(), 0),
                MonitorId::new("qux".to_string(), 0),
            ])
            .unwrap();
        assert_eq!(parent.get_value("count"), Ok(0u64));
    }
}
