pub use hdrhistogram::Histogram;

use comfy_table::{presets, Table};
use hdrhistogram::sync::{Recorder, SyncHistogram};
use indexmap::IndexMap;
use once_cell::sync::Lazy;
use std::{
    borrow::Cow,
    fmt,
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{task::futures::TaskLocalFuture, task_local};

const MAX: Duration = Duration::from_secs(60 * 60);

task_local! {
    static SCOPE: Scope;
}

/// Get or create subscope of the current scope.
pub fn scope(name: impl Into<Cow<'static, str>>) -> Scope {
    let name = name.into();
    SCOPE
        .try_with(|scope| scope.scope(name.clone()))
        .unwrap_or_else(|_| ROOTS.scope(name))
}

pub fn with_scope<F>(name: impl Into<Cow<'static, str>>, f: F) -> TaskLocalFuture<Scope, F>
where
    F: Future,
{
    SCOPE.scope(scope(name), f)
}

pub fn report() -> Report {
    ROOTS.report()
}

pub struct Scope {
    children: Arc<Nodes>,
    recorder: Recorder<u64>,
    start: Instant,
}

impl Scope {
    /// Create sub-scope of this scope.
    pub fn scope(&self, name: impl Into<Cow<'static, str>>) -> Self {
        self.children.scope(name)
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        let value = self.start.elapsed();

        if value
            .as_nanos()
            .try_into()
            .ok()
            .and_then(|value| self.recorder.record(value).ok())
            .is_none()
        {
            tracing::warn!("timing out or range: {:?}", value);
        }
    }
}

pub struct Report(Vec<(Cow<'static, str>, NodeReport)>);

pub struct NodeReport {
    histogram: Histogram<u64>,
    children: Report,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut table = Table::new();

        table.load_preset(presets::UTF8_FULL_CONDENSED);

        table.set_header(vec![
            "name", "count", "min", "max", "mean", "50%", "90%", "99%", "99.9%",
        ]);

        for (name, report) in &self.0 {
            add_to_table(&mut table, name, report, 0);
        }

        write!(f, "{table}")
    }
}

fn add_to_table(table: &mut Table, name: &str, report: &NodeReport, level: usize) {
    let h = &report.histogram;

    let count: u64 = h.len();

    table.add_row(vec![
        format!("{}{}", " ".repeat(level * 2), name),
        format!("{:>5}", count),
        format!("{:.4}s", to_secs(h.min())),
        format!("{:.4}s", to_secs(h.max())),
        format!("{:.4}s", to_secs(h.mean() as u64)),
        format!("{:.4}s", to_secs(h.value_at_quantile(0.5))),
        format!("{:.4}s", to_secs(h.value_at_quantile(0.9))),
        format!("{:.4}s", to_secs(h.value_at_quantile(0.99))),
        format!("{:.4}s", to_secs(h.value_at_quantile(0.999))),
    ]);

    for (name, report) in &report.children.0 {
        add_to_table(table, name, report, level + 1)
    }
}

fn to_secs(nanos: u64) -> f64 {
    nanos as f64 / 1_000_000_000.0
}

static ROOTS: Lazy<Nodes> = Lazy::new(Nodes::default);

#[derive(Default)]
struct Nodes(Mutex<IndexMap<Cow<'static, str>, Node>>);

impl Nodes {
    fn scope(&self, name: impl Into<Cow<'static, str>>) -> Scope {
        self.0
            .lock()
            .unwrap()
            .entry(name.into())
            .or_default()
            .scope()
    }

    fn report(&self) -> Report {
        Report(
            self.0
                .lock()
                .unwrap()
                .iter_mut()
                .map(|(name, node)| (name.clone(), node.report()))
                .collect(),
        )
    }
}

struct Node {
    histogram: SyncHistogram<u64>,
    children: Arc<Nodes>,
}

impl Node {
    fn scope(&self) -> Scope {
        let children = self.children.clone();
        let recorder = self.histogram.recorder();

        Scope {
            children,
            recorder,
            start: Instant::now(),
        }
    }

    fn report(&mut self) -> NodeReport {
        self.histogram.refresh();
        let histogram = self.histogram.clone();
        let children = self.children.report();

        NodeReport {
            histogram,
            children,
        }
    }
}

impl Default for Node {
    fn default() -> Self {
        Self {
            histogram: Histogram::new_with_max(MAX.as_nanos().try_into().unwrap(), 2)
                .unwrap()
                .into(),
            children: Arc::new(Nodes::default()),
        }
    }
}
