pub use hdrhistogram::Histogram;

use indexmap::IndexMap;
use std::{
    borrow::Cow,
    thread,
    time::{Duration, Instant},
};

const MAX_TIME: Duration = Duration::from_secs(60 * 60);
const MAX_THROUGHPUT: u64 = 1_000_000;

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
    /// Records a duration directly
    pub fn record(&self, duration: Duration) {
        self.tx
            .send(Command::Record {
                name: self.name.clone(),
                value: Value {
                    duration,
                    timestamp: Instant::now(),
                },
            })
            .ok();
    }

    /// Starts measuring the duration of a section of code using this metric. The measuring stops
    /// and the measured duration is recorded when the returned `Timing` goes out of scope.
    pub fn start(&self) -> Timing {
        Timing {
            metric: self,
            start: Instant::now(),
        }
    }
}

pub struct Timing<'a> {
    metric: &'a Metric,
    start: Instant,
}

impl Drop for Timing<'_> {
    fn drop(&mut self) {
        self.metric.record(self.start.elapsed())
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
            time_histogram: &report.time_histogram,
            throughput_histogram: &report.throughput_histogram,
        })
    }

    fn register(&mut self, name: MetricName) {
        self.metrics.entry(name).or_default();
    }

    fn record(&mut self, name: MetricName, value: Value) {
        if let Some(report) = self.metrics.get_mut(&name) {
            report.record(value);
        }
    }
}

pub struct ReportItem<'a> {
    pub name: &'a MetricName,
    pub time_histogram: &'a Histogram<u64>,
    pub throughput_histogram: &'a Histogram<u64>,
}

const PERIOD: Duration = Duration::from_secs(1);

struct MetricReport {
    period_start: Option<Instant>,
    time_histogram: Histogram<u64>,
    throughput_histogram: Histogram<u64>,
    throughput_count: u64,
}

impl MetricReport {
    fn record(&mut self, value: Value) {
        self.time_histogram
            .saturating_record(value.duration.as_nanos().try_into().unwrap_or(u64::MAX));

        if let Some(period_start) = self.period_start {
            if value.timestamp.duration_since(period_start) > PERIOD {
                self.throughput_histogram
                    .saturating_record(self.throughput_count);
                self.throughput_count = 0;
                self.period_start = Some(value.timestamp);
            }
        } else {
            self.period_start = Some(value.timestamp);
        }

        self.throughput_count += 1;
    }
}

impl Default for MetricReport {
    fn default() -> Self {
        Self {
            period_start: None,
            time_histogram: Histogram::new_with_max(MAX_TIME.as_nanos().try_into().unwrap(), 2)
                .unwrap(),
            throughput_histogram: Histogram::new_with_max(MAX_THROUGHPUT, 2).unwrap(),
            throughput_count: 0,
        }
    }
}

struct Value {
    duration: Duration,
    timestamp: Instant,
}

enum Command {
    Register(MetricName),
    Record {
        name: MetricName,
        value: Value,
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
