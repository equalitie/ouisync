pub use hdrhistogram::Histogram;

use indexmap::IndexMap;
use std::{
    borrow::Cow,
    future::Future,
    thread,
    time::{Duration, Instant},
};

const MAX_TIME: Duration = Duration::from_secs(60 * 60);

#[derive(Clone)]
pub struct Metrics {
    tx: crossbeam_channel::Sender<Command>,
}

impl Metrics {
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        thread::spawn(move || run(rx));

        Self { tx }
    }

    pub fn get(&self, name: impl Into<MetricName>) -> Metric {
        let name = name.into();

        self.tx.send(Command::Register(name.clone())).ok();

        Metric {
            name,
            tx: self.tx.clone(),
        }
    }

    pub fn report<F>(&self, reporter: F)
    where
        F: FnOnce(&Report) + Send + 'static,
    {
        self.tx
            .send(Command::Report {
                reporter: Box::new(reporter),
            })
            .ok();
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct Metric {
    name: MetricName,
    tx: crossbeam_channel::Sender<Command>,
}

impl Metric {
    pub fn record(&self, value: Duration) {
        self.tx
            .send(Command::Record {
                name: self.name.clone(),
                value,
            })
            .ok();
    }

    pub fn start(&self) -> Timing<'_> {
        Timing {
            metric: self,
            start: Instant::now(),
        }
    }

    /// Measures the duration of running the given future and if it returns `Ok`, record it.
    pub async fn measure_ok<F, T, E>(&self, f: F) -> F::Output
    where
        F: Future<Output = Result<T, E>>,
    {
        let start = Instant::now();
        let output = f.await;

        if output.is_ok() {
            self.record(start.elapsed());
        }

        output
    }

    pub fn name(&self) -> &MetricName {
        &self.name
    }
}

pub struct Timing<'a> {
    metric: &'a Metric,
    start: Instant,
}

impl Drop for Timing<'_> {
    fn drop(&mut self) {
        self.metric.record(self.start.elapsed());
    }
}

pub type MetricName = Cow<'static, str>;

#[derive(Default)]
pub struct Report {
    metrics: IndexMap<MetricName, MetricReport>,
}

impl Report {
    pub fn items(&self) -> impl Iterator<Item = ReportItem<'_>> + ExactSizeIterator {
        self.metrics.iter().map(|(name, report)| ReportItem {
            name,
            time: TimeView(report),
            throughput: ThroughputView(report),
        })
    }

    fn register(&mut self, name: MetricName) {
        self.metrics.entry(name).or_default();
    }

    fn record(&mut self, name: MetricName, value: Duration) {
        if let Some(report) = self.metrics.get_mut(&name) {
            report.record(value);
        }
    }
}

pub struct ReportItem<'a> {
    pub name: &'a MetricName,
    pub time: TimeView<'a>,
    pub throughput: ThroughputView<'a>,
}

pub struct TimeView<'a>(&'a MetricReport);

impl TimeView<'_> {
    pub fn count(&self) -> u64 {
        self.0.histogram.len()
    }

    pub fn last(&self) -> Duration {
        Duration::from_nanos(self.0.last)
    }

    pub fn min(&self) -> Duration {
        Duration::from_nanos(self.0.histogram.min())
    }

    pub fn max(&self) -> Duration {
        Duration::from_nanos(self.0.histogram.max())
    }

    pub fn mean(&self) -> Duration {
        Duration::from_nanos(self.0.histogram.mean().round() as u64)
    }

    pub fn stdev(&self) -> Duration {
        Duration::from_nanos(self.0.histogram.stdev().round() as u64)
    }

    pub fn value_at_quantile(&self, quantile: f64) -> Duration {
        Duration::from_nanos(self.0.histogram.value_at_quantile(quantile))
    }
}

pub struct ThroughputView<'a>(&'a MetricReport);

impl ThroughputView<'_> {
    pub fn count(&self) -> u64 {
        self.0.histogram.len()
    }

    pub fn last(&self) -> f64 {
        throughput(self.0.last)
    }

    pub fn min(&self) -> f64 {
        throughput(self.0.histogram.max())
    }

    pub fn max(&self) -> f64 {
        throughput(self.0.histogram.min())
    }

    pub fn mean(&self) -> f64 {
        // Modified version of https://docs.rs/hdrhistogram/latest/hdrhistogram/struct.Histogram.html#method.mean
        let h = &self.0.histogram;

        if h.is_empty() {
            return 0.0;
        }

        h.iter_recorded().fold(0.0, |total, v| {
            total
                + throughput(h.median_equivalent(v.value_iterated_to())) * v.count_at_value() as f64
                    / h.len() as f64
        })
    }

    pub fn stdev(&self) -> f64 {
        // Modified version of https://docs.rs/hdrhistogram/latest/hdrhistogram/struct.Histogram.html#method.stdev
        let h = &self.0.histogram;

        if h.is_empty() {
            return 0.0;
        }

        let mean = self.mean();
        let geom_dev_tot = h.iter_recorded().fold(0.0, |gdt, v| {
            let dev = throughput(h.median_equivalent(v.value_iterated_to())) - mean;
            gdt + (dev * dev) * v.count_since_last_iteration() as f64
        });

        (geom_dev_tot / h.len() as f64).sqrt()
    }

    pub fn value_at_quantile(&self, quantile: f64) -> f64 {
        throughput(self.0.histogram.value_at_quantile(1.0 - quantile))
    }
}

fn throughput(nanos: u64) -> f64 {
    if nanos == 0 {
        0.0
    } else {
        1.0 / Duration::from_nanos(nanos).as_secs_f64()
    }
}

struct MetricReport {
    last: u64,
    histogram: Histogram<u64>,
}

impl MetricReport {
    fn record(&mut self, value: Duration) {
        self.last = value.as_nanos().try_into().unwrap_or(u64::MAX);
        self.histogram.saturating_record(self.last);
    }
}

impl Default for MetricReport {
    fn default() -> Self {
        Self {
            last: 0,
            histogram: Histogram::new_with_max(MAX_TIME.as_nanos().try_into().unwrap(), 2).unwrap(),
        }
    }
}

enum Command {
    Register(MetricName),
    Record {
        name: MetricName,
        value: Duration,
    },
    Report {
        reporter: Box<dyn FnOnce(&Report) + Send + 'static>,
    },
}

fn run(rx: crossbeam_channel::Receiver<Command>) {
    let mut report = Report::default();

    for command in rx {
        match command {
            Command::Register(name) => report.register(name),
            Command::Record { name, value } => report.record(name, value),
            Command::Report { reporter } => {
                reporter(&report);
            }
        }
    }
}
