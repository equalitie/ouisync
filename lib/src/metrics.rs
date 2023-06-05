pub use hdrhistogram::Histogram;

use indexmap::IndexMap;
use std::{
    borrow::Cow,
    future::Future,
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
            time_histogram: &report.time_histogram,
            time_recent: report.time_recent.get(),
            throughput_histogram: &report.throughput_histogram,
            throughput_recent: report.throughput_recent.get(),
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
    pub time_recent: f64,
    pub throughput_histogram: &'a Histogram<u64>,
    pub throughput_recent: f64,
}

const PERIOD: Duration = Duration::from_secs(1);
const RECENT_PERIODS: usize = 5;

struct MetricReport {
    period_start: Option<Instant>,
    time_histogram: Histogram<u64>,
    time_recent: MovingAverage<RECENT_PERIODS>,
    throughput_histogram: Histogram<u64>,
    throughput_recent: MovingAverage<RECENT_PERIODS>,
    throughput_count: u64,
}

impl MetricReport {
    fn record(&mut self, value: Value) {
        let time = value.duration.as_nanos().try_into().unwrap_or(u64::MAX);
        self.time_histogram.saturating_record(time);
        self.time_recent.record(time);

        if let Some(period_start) = self.period_start {
            if value.timestamp.duration_since(period_start) > PERIOD {
                self.throughput_histogram
                    .saturating_record(self.throughput_count);
                self.throughput_recent.record(self.throughput_count);

                self.throughput_count = 0;
                self.period_start = Some(value.timestamp);

                self.time_recent.advance();
                self.throughput_recent.advance();
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
            time_recent: MovingAverage::default(),
            throughput_histogram: Histogram::new_with_max(MAX_THROUGHPUT, 2).unwrap(),
            throughput_recent: MovingAverage::default(),
            throughput_count: 0,
        }
    }
}

/// Maintains a moving average of some value during the last N time periods (typically seconds).
struct MovingAverage<const N: usize> {
    slots: [Slot; N],
    curr: usize,
}

impl<const N: usize> MovingAverage<N> {
    /// Records a new value in the current time period.
    pub fn record(&mut self, value: u64) {
        let slot = &mut self.slots[self.curr];

        slot.sum += value;
        slot.count += 1;
    }

    /// Starts a new time period.
    pub fn advance(&mut self) {
        let next = (self.curr + 1) % self.slots.len();
        self.slots[next] = Slot::default();
        self.curr = next;
    }

    /// Retrieves the current average value.
    pub fn get(&self) -> f64 {
        let total = self.slots.iter().fold(Slot::default(), |total, slot| Slot {
            sum: total.sum + slot.sum,
            count: total.count + slot.count,
        });

        if total.count > 0 {
            total.sum as f64 / total.count as f64
        } else {
            0.0
        }
    }
}

impl<const N: usize> Default for MovingAverage<N> {
    fn default() -> Self {
        Self {
            slots: [Slot::default(); N],
            curr: 0,
        }
    }
}

#[derive(Copy, Clone, Default)]
struct Slot {
    sum: u64,
    count: u64,
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
