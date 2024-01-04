use crate::{MonitoredValue, StateMonitor};
use metrics::{
    Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, Key, KeyName, Metadata, Recorder,
    SharedString, Unit,
};
use metrics_util::Summary;
use std::{
    borrow::Cow,
    collections::HashMap,
    fmt,
    ops::{Deref, DerefMut},
    sync::{Arc, Mutex},
};

/// Adapter for StateMonitor that allows it to be used as a
/// [metrics](https://crates.io/crates/metrics)
/// [`Recorder`](https://docs.rs/metrics/latest/metrics/trait.Recorder.html).
pub struct MetricsRecorder {
    parent_node: StateMonitor,
    descriptions: Mutex<HashMap<KeyName, Unit>>,
}

impl MetricsRecorder {
    pub fn new(parent_node: StateMonitor) -> Self {
        Self {
            parent_node,
            descriptions: Mutex::new(HashMap::default()),
        }
    }

    fn describe(&self, key: KeyName, unit: Option<Unit>) {
        let mut descriptions = self.descriptions.lock().unwrap();

        if let Some(unit) = unit {
            descriptions.insert(key, unit);
        } else {
            descriptions.remove(&key);
        }
    }
}

impl Recorder for MetricsRecorder {
    fn describe_counter(&self, key: KeyName, unit: Option<Unit>, _description: SharedString) {
        self.describe(key, unit);
    }

    fn describe_gauge(&self, key: KeyName, unit: Option<Unit>, _description: SharedString) {
        self.describe(key, unit);
    }

    fn describe_histogram(&self, key: KeyName, unit: Option<Unit>, _description: SharedString) {
        self.describe(key, unit);
    }

    fn register_counter(&self, key: &Key, _metadata: &Metadata<'_>) -> Counter {
        let unit = self.descriptions.lock().unwrap().get(key.name()).copied();
        let value = make_value_recursive(&self.parent_node, key.name(), Formatted(0u64, unit));
        Counter::from_arc(Arc::new(value))
    }

    fn register_gauge(&self, key: &Key, _metadata: &Metadata<'_>) -> Gauge {
        let unit = self.descriptions.lock().unwrap().get(key.name()).copied();
        let value = make_value_recursive(&self.parent_node, key.name(), Formatted(0.0, unit));

        Gauge::from_arc(Arc::new(value))
    }

    fn register_histogram(&self, key: &Key, _metadata: &Metadata<'_>) -> Histogram {
        let unit = self.descriptions.lock().unwrap().get(key.name()).copied();
        let node = HistogramNode::new(
            make_node_recursive(&self.parent_node, key.name()).into_owned(),
            unit,
        );

        Histogram::from_arc(Arc::new(node))
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

impl CounterFn for MonitoredValue<Formatted<u64>> {
    fn increment(&self, value: u64) {
        **self.get() += value;
    }

    fn absolute(&self, value: u64) {
        let mut v = self.get();
        **v = v.max(value);
    }
}

impl GaugeFn for MonitoredValue<Formatted<f64>> {
    fn increment(&self, value: f64) {
        **self.get() += value;
    }

    fn decrement(&self, value: f64) {
        **self.get() -= value;
    }

    fn set(&self, value: f64) {
        **self.get() = value;
    }
}

struct HistogramNode {
    summary: Mutex<Summary>,
    count: MonitoredValue<u64>,
    min: MonitoredValue<Formatted<f64>>,
    max: MonitoredValue<Formatted<f64>>,
    p50: MonitoredValue<Formatted<f64>>,
    p90: MonitoredValue<Formatted<f64>>,
    p99: MonitoredValue<Formatted<f64>>,
    p999: MonitoredValue<Formatted<f64>>,
}

impl HistogramNode {
    fn new(node: StateMonitor, unit: Option<Unit>) -> Self {
        Self {
            summary: Mutex::new(Summary::with_defaults()),
            count: node.make_value("count", 0),
            min: node.make_value("min", Formatted(0.0, unit)),
            max: node.make_value("max", Formatted(0.0, unit)),
            p50: node.make_value("50%", Formatted(0.0, unit)),
            p90: node.make_value("90%", Formatted(0.0, unit)),
            p99: node.make_value("99%", Formatted(0.0, unit)),
            p999: node.make_value("99.9%", Formatted(0.0, unit)),
        }
    }
}

impl HistogramFn for HistogramNode {
    fn record(&self, value: f64) {
        let mut summary = self.summary.lock().unwrap();

        summary.add(value);

        *self.count.get() = summary.count() as u64;
        **self.min.get() = summary.min();
        **self.max.get() = summary.max();

        **self.p50.get() = summary.quantile(0.5).unwrap_or(0.0);
        **self.p90.get() = summary.quantile(0.9).unwrap_or(0.0);
        **self.p99.get() = summary.quantile(0.99).unwrap_or(0.0);
        **self.p999.get() = summary.quantile(0.999).unwrap_or(0.0);
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

        let counter = recorder.register_counter(
            &Key::from_name("test_counter"),
            &Metadata::new(module_path!(), Level::INFO, None),
        );

        assert_eq!(root.get_value("test_counter"), Ok(Formatted(0u64, None)));

        counter.increment(1);

        assert_eq!(root.get_value("test_counter"), Ok(Formatted(1u64, None)));
    }

    #[tokio::test]
    async fn gauge() {
        let root = StateMonitor::make_root();
        let recorder = MetricsRecorder::new(root.clone());

        let gauge = recorder.register_gauge(
            &Key::from_name("test_gauge"),
            &Metadata::new(module_path!(), Level::INFO, None),
        );

        assert_eq!(root.get_value("test_gauge"), Ok(Formatted(0.0, None)));

        gauge.increment(2.0);
        assert_eq!(root.get_value("test_gauge"), Ok(Formatted(2.0, None)));

        gauge.decrement(1.0);
        assert_eq!(root.get_value("test_gauge"), Ok(Formatted(1.0, None)));

        gauge.set(4.0);
        assert_eq!(root.get_value("test_gauge"), Ok(Formatted(4.0, None)));
    }

    #[tokio::test]
    async fn histogram() {
        let root = StateMonitor::make_root();
        let recorder = MetricsRecorder::new(root.clone());

        let histogram = recorder.register_histogram(
            &Key::from_name("test_histogram"),
            &Metadata::new(module_path!(), Level::INFO, None),
        );

        let parent = root
            .locate([MonitorId::new("test_histogram".to_string(), 0)])
            .unwrap();

        assert_eq!(parent.get_value("count"), Ok(0u64));
        assert_eq!(parent.get_value("min"), Ok(Formatted(0.0, None)));
        assert_eq!(parent.get_value("max"), Ok(Formatted(0.0, None)));

        histogram.record(1.0);

        assert_eq!(parent.get_value("count"), Ok(1u64));
        assert_eq!(parent.get_value("min"), Ok(Formatted(1.0, None)));
        assert_eq!(parent.get_value("max"), Ok(Formatted(1.0, None)));

        histogram.record(2.0);

        assert_eq!(parent.get_value("count"), Ok(2u64));
        assert_eq!(parent.get_value("min"), Ok(Formatted(1.0, None)));
        assert_eq!(parent.get_value("max"), Ok(Formatted(2.0, None)));
    }

    #[tokio::test]
    async fn description() {
        let root = StateMonitor::make_root();
        let recorder = MetricsRecorder::new(root.clone());

        let name = KeyName::from("test_counter");

        recorder.describe_counter(name.clone(), Some(Unit::Seconds), "".into());
        let _counter = recorder.register_counter(
            &Key::from_name(name),
            &Metadata::new(module_path!(), Level::INFO, None),
        );

        assert_eq!(
            root.get_value("test_counter"),
            Ok(Formatted(0u64, Some(Unit::Seconds)))
        );
    }

    #[tokio::test]
    async fn unregister() {
        let root = StateMonitor::make_root();
        let recorder = MetricsRecorder::new(root.clone());

        let counter = recorder.register_counter(
            &Key::from_name("test_counter"),
            &Metadata::new(module_path!(), Level::INFO, None),
        );

        assert_eq!(
            root.get_value::<Formatted<u64>>("test_counter"),
            Ok(Formatted(0, None))
        );

        drop(counter);

        assert_eq!(
            root.get_value::<Formatted<u64>>("test_counter"),
            Err(ValueError::NotFound)
        );
    }

    #[tokio::test]
    async fn hierarchy() {
        let root = StateMonitor::make_root();
        let recorder = MetricsRecorder::new(root.clone());

        let _counter = recorder.register_counter(
            &Key::from_name("foo/bar/baz"),
            &Metadata::new(module_path!(), Level::INFO, None),
        );

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
