pub use hdrhistogram::Histogram;

use indexmap::IndexMap;
use std::{
    borrow::Cow,
    thread,
    time::{Duration, Instant},
};

const MAX: Duration = Duration::from_secs(60 * 60);

#[derive(Clone)]
pub struct Clocks {
    tx: crossbeam_channel::Sender<Command>,
}

impl Clocks {
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        thread::spawn(move || run(rx));

        Self { tx }
    }

    /// Creates a clock
    pub fn clock(&self, name: impl Into<ClockName>) -> Clock {
        Clock::new(name.into(), self.tx.clone())
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

impl Default for Clocks {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct Clock {
    name: ClockName,
    tx: crossbeam_channel::Sender<Command>,
}

impl Clock {
    fn new(name: ClockName, tx: crossbeam_channel::Sender<Command>) -> Self {
        tx.send(Command::Register { name: name.clone() }).ok();

        Self { name, tx }
    }

    /// Records a value directly
    pub fn record(&self, value: Duration) {
        self.tx
            .send(Command::Record {
                name: self.name.clone(),
                value,
            })
            .ok();
    }

    /// Starts measuring the duration of a section of code using this clock. The measuring stops
    /// and the measured duration is recorded when the returned `Recording` goes out of scope.
    pub fn start(&self) -> Recording {
        Recording {
            clock: self,
            start: Instant::now(),
        }
    }
}

pub struct Recording<'a> {
    clock: &'a Clock,
    start: Instant,
}

impl Drop for Recording<'_> {
    fn drop(&mut self) {
        self.clock.record(self.start.elapsed())
    }
}

pub type ClockName = Cow<'static, str>;

#[derive(Default)]
pub struct Report {
    nodes: IndexMap<ClockName, Node>,
}

impl Report {
    pub fn items(&self) -> impl Iterator<Item = ReportItem<'_>> {
        self.nodes
            .iter()
            .map(|(name, node)| ReportItem { name, node })
    }

    fn register(&mut self, name: ClockName) {
        self.nodes.insert(name, Node::new());
    }

    fn record(&mut self, name: ClockName, value: Duration) {
        let Some(node) = self.nodes.get_mut(&name) else {
            return;
        };

        if value
            .as_nanos()
            .try_into()
            .ok()
            .and_then(|value| node.histogram.record(value).ok())
            .is_none()
        {
            tracing::warn!("timing out or range: {:?}", value);
        }
    }
}

pub struct ReportItem<'a> {
    name: &'a ClockName,
    node: &'a Node,
}

impl<'a> ReportItem<'a> {
    pub fn name(&self) -> &'a ClockName {
        self.name
    }

    pub fn histogram(&self) -> &'a Histogram<u64> {
        &self.node.histogram
    }
}

struct Node {
    histogram: Histogram<u64>,
}

impl Node {
    fn new() -> Self {
        Self {
            histogram: Histogram::new_with_max(MAX.as_nanos().try_into().unwrap(), 2).unwrap(),
        }
    }
}

enum Command {
    Register {
        name: ClockName,
    },
    Record {
        name: ClockName,
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
            Command::Register { name } => report.register(name),
            Command::Record { name, value } => report.record(name, value),
            Command::Report { reporter } => {
                reporter(&report);
            }
        }
    }
}
