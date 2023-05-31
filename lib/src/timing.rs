pub use hdrhistogram::Histogram;

use indexmap::IndexMap;
use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const MAX: Duration = Duration::from_secs(60 * 60);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct Clocks {
    tx: crossbeam_channel::Sender<Command>,
    roots: Arc<ClockMap>,
}

impl Clocks {
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        thread::spawn(move || run(rx));

        Self {
            tx,
            roots: Arc::new(ClockMap::default()),
        }
    }

    /// Creates a sub-clock
    pub fn clock(&self, name: impl Into<ClockName>) -> Clock {
        self.roots.fetch(name.into(), 0, self.tx.clone())
    }

    pub fn report<F>(&self, reporter: F) -> ReportHandle
    where
        F: FnOnce(&Report) + Send + 'static,
    {
        let (complete_tx, complete_rx) = crossbeam_channel::bounded(0);

        self.tx
            .send(Command::Report {
                reporter: Box::new(reporter),
                complete_tx,
            })
            .ok();

        ReportHandle(complete_rx)
    }
}

impl Default for Clocks {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct Clock {
    tx: crossbeam_channel::Sender<Command>,
    id: u64,
    children: Arc<ClockMap>,
}

impl Clock {
    fn new(name: ClockName, parent_id: u64, tx: crossbeam_channel::Sender<Command>) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

        tx.send(Command::Register {
            name,
            parent_id,
            id,
        })
        .ok();

        Self {
            tx,
            id,
            children: Arc::new(ClockMap::default()),
        }
    }

    /// Creates a sub-clock
    pub fn clock(&self, name: impl Into<ClockName>) -> Self {
        self.children.fetch(name.into(), self.id, self.tx.clone())
    }

    pub fn start(&self) -> Recording {
        Recording {
            clock: Cow::Borrowed(self),
            start: Instant::now(),
        }
    }

    pub fn started(self) -> Recording<'static> {
        Recording {
            clock: Cow::Owned(self),
            start: Instant::now(),
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }
}

#[derive(Default)]
struct ClockMap(Mutex<HashMap<ClockName, Clock>>);

impl ClockMap {
    fn fetch(
        &self,
        name: ClockName,
        parent_id: u64,
        tx: crossbeam_channel::Sender<Command>,
    ) -> Clock {
        self.0
            .lock()
            .unwrap()
            .entry(name.clone())
            .or_insert_with(|| Clock::new(name, parent_id, tx))
            .clone()
    }
}

pub struct Recording<'a> {
    clock: Cow<'a, Clock>,
    start: Instant,
}

impl Drop for Recording<'_> {
    fn drop(&mut self) {
        self.clock
            .tx
            .send(Command::Record {
                id: self.clock.id,
                value: self.start.elapsed(),
            })
            .ok();
    }
}

pub struct ReportHandle(crossbeam_channel::Receiver<()>);

impl ReportHandle {
    /// Blocks until the reporting is complete.
    pub fn wait(self) {
        self.0.recv().ok();
    }
}

pub type ClockName = Cow<'static, str>;

#[derive(Default)]
pub struct Report {
    nodes: HashMap<u64, Node>,
    roots: IndexMap<ClockName, u64>,
}

impl Report {
    pub fn items(&self) -> impl Iterator<Item = ReportItem<'_>> {
        self.iter_with(&self.roots)
    }

    fn iter_with<'a>(
        &'a self,
        index: &'a IndexMap<ClockName, u64>,
    ) -> impl Iterator<Item = ReportItem<'a>> {
        index.iter().filter_map(|(name, id)| {
            let node = self.nodes.get(id)?;

            Some(ReportItem {
                report: self,
                name,
                node,
            })
        })
    }

    fn register(&mut self, name: ClockName, parent_id: u64, id: u64) {
        self.nodes.insert(id, Node::new());

        if parent_id == 0 {
            self.roots.insert(name, id);
        } else {
            self.nodes
                .get_mut(&parent_id)
                .expect("missing parent node")
                .children
                .insert(name, id);
        }
    }

    fn record(&mut self, id: u64, value: Duration) {
        let Some(node) = self.nodes.get_mut(&id) else {
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
    report: &'a Report,
    name: &'a ClockName,
    node: &'a Node,
}

impl<'a> ReportItem<'a> {
    pub fn name(&self) -> &'a str {
        self.name.as_ref()
    }

    pub fn histogram(&self) -> &'a Histogram<u64> {
        &self.node.histogram
    }

    pub fn items(&self) -> impl Iterator<Item = ReportItem<'a>> {
        self.report.iter_with(&self.node.children)
    }
}

struct Node {
    histogram: Histogram<u64>,
    children: IndexMap<ClockName, u64>,
}

impl Node {
    fn new() -> Self {
        Self {
            histogram: Histogram::new_with_max(MAX.as_nanos().try_into().unwrap(), 2).unwrap(),
            children: IndexMap::new(),
        }
    }
}

enum Command {
    Register {
        name: ClockName,
        parent_id: u64,
        id: u64,
    },
    Record {
        id: u64,
        value: Duration,
    },
    Report {
        reporter: Box<dyn FnOnce(&Report) + Send + 'static>,
        complete_tx: crossbeam_channel::Sender<()>,
    },
}

fn run(rx: crossbeam_channel::Receiver<Command>) {
    let mut report = Report::default();

    for command in rx {
        match command {
            Command::Register {
                name,
                parent_id,
                id,
            } => report.register(name, parent_id, id),
            Command::Record { id, value } => report.record(id, value),
            Command::Report {
                reporter,
                complete_tx: _complete_tx,
            } => {
                reporter(&report);
            }
        }
    }
}
