pub use hdrhistogram::Histogram;

use indexmap::IndexMap;
use std::{
    borrow::Cow,
    future::Future,
    iter, mem, thread,
    time::{Duration, Instant},
};
use tokio::{task::futures::TaskLocalFuture, task_local};

const MAX: Duration = Duration::from_secs(60 * 60);

task_local! {
    static TIMER: Timer;
    static SCOPE: Scope;
}

pub fn scope(name: impl Into<Name>) -> Scope {
    let name = name.into();
    SCOPE
        .try_with(|scope| scope.scope(name.clone()))
        .unwrap_or_else(|_| TIMER.with(|timer| timer.scope(name)))
}

#[derive(Clone)]
pub struct Timer {
    tx: crossbeam_channel::Sender<Command>,
}

impl Timer {
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();

        thread::spawn(move || run(rx));

        Self { tx }
    }

    /// Creates a top-level timer scope
    pub fn scope(&self, name: impl Into<Name>) -> Scope {
        Scope::new(vec![name.into()], self.tx.clone())
    }

    pub fn report<F>(&self, reporter: F) -> ReportHandle
    where
        F: FnOnce(&Root) + Send + 'static,
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

    /// Sets this timer as the default timer for the given future.
    pub fn apply<F>(self, f: F) -> TaskLocalFuture<Self, F>
    where
        F: Future,
    {
        TIMER.scope(self, f)
    }
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Scope {
    path: Vec<Name>,
    tx: crossbeam_channel::Sender<Command>,
    start: Instant,
}

impl Scope {
    fn new(path: Vec<Name>, tx: crossbeam_channel::Sender<Command>) -> Self {
        Self {
            path,
            tx,
            start: Instant::now(),
        }
    }

    /// Creates a subscope of this scope
    pub fn scope(&self, name: impl Into<Name>) -> Self {
        Self::new(
            self.path
                .iter()
                .cloned()
                .chain(iter::once(name.into()))
                .collect(),
            self.tx.clone(),
        )
    }

    /// Runs the future in this scope
    pub fn apply<F>(self, f: F) -> TaskLocalFuture<Self, F>
    where
        F: Future,
    {
        SCOPE.scope(self, f)
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        self.tx
            .send(Command::Record {
                path: mem::take(&mut self.path),
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

pub type Name = Cow<'static, str>;

#[derive(Default)]
pub struct Root {
    pub children: IndexMap<Name, Node>,
}

impl Root {
    fn fetch(&mut self, path: Vec<Name>) -> &mut Node {
        let mut path = path.into_iter();

        let name = path.next().unwrap();
        let mut current = self.children.entry(name).or_default();

        for name in path {
            current = current.children.entry(name).or_default();
        }

        current
    }
}

pub struct Node {
    pub histogram: Histogram<u64>,
    pub children: IndexMap<Name, Self>,
}

impl Node {
    fn new() -> Self {
        Self {
            histogram: Histogram::new_with_max(MAX.as_nanos().try_into().unwrap(), 2).unwrap(),
            children: Default::default(),
        }
    }
}

impl Default for Node {
    fn default() -> Self {
        Self::new()
    }
}

enum Command {
    Record {
        path: Vec<Name>,
        value: Duration,
    },
    Report {
        reporter: Box<dyn FnOnce(&Root) + Send + 'static>,
        complete_tx: crossbeam_channel::Sender<()>,
    },
}

fn run(rx: crossbeam_channel::Receiver<Command>) {
    let mut root = Root::default();

    for command in rx {
        match command {
            Command::Record { path, value } => {
                let node = root.fetch(path);

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
            Command::Report {
                reporter,
                complete_tx: _complete_tx,
            } => {
                reporter(&root);
            }
        }
    }
}
