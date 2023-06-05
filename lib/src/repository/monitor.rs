use std::{fmt, time::Duration};

use crate::{
    collections::HashMap,
    state_monitor::{MonitoredValue, StateMonitor},
    timing::{Clock, Clocks, Report, ReportItem},
};
use btdht::InfoHash;
use scoped_task::ScopedJoinHandle;
use tokio::{sync::oneshot, time};
use tracing::Span;

pub(crate) struct RepositoryMonitor {
    // This indicates how many requests for index nodes are currently in flight.  It is used by the
    // UI to indicate that the index is being synchronized.
    pub index_requests_inflight: MonitoredValue<u64>,
    pub block_requests_inflight: MonitoredValue<u64>,
    pub pending_requests: MonitoredValue<u64>,
    pub total_requests_cummulative: MonitoredValue<u64>,
    pub request_timeouts: MonitoredValue<u64>,
    pub info_hash: MonitoredValue<Option<InfoHash>>,

    pub clock_handle_root_node: Clock,
    pub clock_handle_inner_nodes: Clock,
    pub clock_handle_leaf_nodes: Clock,
    pub clock_handle_block: Clock,
    pub clock_request_queued: Clock,
    pub clock_request_inflight: Clock,

    span: Span,
    node: StateMonitor,
    _report_clocks_task: ScopedJoinHandle<()>,
}

impl RepositoryMonitor {
    pub fn new(parent: StateMonitor, clocks: Clocks, name: &str) -> Self {
        let span = tracing::info_span!("repo", name);
        let node = parent.make_child(name);

        let clock_handle_root_node = clocks.clock("handle_root_node");
        let clock_handle_inner_nodes = clocks.clock("handle_inner_node");
        let clock_handle_leaf_nodes = clocks.clock("handle_leaf_node");
        let clock_handle_block = clocks.clock("handle_block");
        let clock_request_queued = clocks.clock("request queued");
        let clock_request_inflight = clocks.clock("request inflight");

        let report_clocks_task = scoped_task::spawn(report_clocks(clocks, node.clone()));

        Self {
            index_requests_inflight: node.make_value("index requests inflight", 0),
            block_requests_inflight: node.make_value("block requests inflight", 0),
            pending_requests: node.make_value("pending requests", 0),
            total_requests_cummulative: node.make_value("total requests cummulative", 0),
            request_timeouts: node.make_value("request timeouts", 0),
            info_hash: node.make_value("info-hash", None),

            clock_handle_root_node,
            clock_handle_inner_nodes,
            clock_handle_leaf_nodes,
            clock_handle_block,
            clock_request_queued,
            clock_request_inflight,

            span,
            node,
            _report_clocks_task: report_clocks_task,
        }
    }

    pub fn span(&self) -> &Span {
        &self.span
    }

    pub fn node(&self) -> &StateMonitor {
        &self.node
    }

    pub fn name(&self) -> &str {
        self.node.id().name()
    }
}

async fn report_clocks(clocks: Clocks, monitor: StateMonitor) {
    let mut interval = time::interval(Duration::from_secs(1));
    let mut monitors = HashMap::new();

    loop {
        interval.tick().await;

        let (tx, rx) = oneshot::channel();
        let monitor = monitor.clone();

        clocks.report(move |report: &Report| {
            for item in report.items() {
                monitors
                    .entry(item.name().clone())
                    .or_insert_with(|| ClockMonitor::new(monitor.make_child(item.name().as_ref())))
                    .update(&item);
            }

            tx.send(monitors).ok();
        });

        monitors = rx.await.unwrap();
    }
}

struct ClockMonitor {
    count: MonitoredValue<u64>,
    mean: MonitoredValue<Seconds>,
    max: MonitoredValue<Seconds>,
    p50: MonitoredValue<Seconds>,
    p90: MonitoredValue<Seconds>,
    p99: MonitoredValue<Seconds>,
    p999: MonitoredValue<Seconds>,
}

impl ClockMonitor {
    fn new(node: StateMonitor) -> Self {
        Self {
            count: node.make_value("count", 0),
            mean: node.make_value("mean", Seconds::ZERO),
            max: node.make_value("max", Seconds::ZERO),
            p50: node.make_value("50%", Seconds::ZERO),
            p90: node.make_value("90%", Seconds::ZERO),
            p99: node.make_value("99%", Seconds::ZERO),
            p999: node.make_value("99.9%", Seconds::ZERO),
        }
    }

    fn update(&self, report: &ReportItem<'_>) {
        let h = report.histogram();

        *self.count.get() = h.len();
        *self.mean.get() = Seconds::from_nanos_f(h.mean());
        *self.max.get() = Seconds::from_nanos(h.max());
        *self.p50.get() = Seconds::from_nanos(h.value_at_quantile(0.5));
        *self.p90.get() = Seconds::from_nanos(h.value_at_quantile(0.9));
        *self.p99.get() = Seconds::from_nanos(h.value_at_quantile(0.99));
        *self.p999.get() = Seconds::from_nanos(h.value_at_quantile(0.999));
    }
}

struct Seconds(f64);

impl Seconds {
    const ZERO: Self = Self(0.0);

    fn from_nanos(n: u64) -> Self {
        Self::from_nanos_f(n as f64)
    }

    fn from_nanos_f(n: f64) -> Self {
        Self(n / 1_000_000_000.0)
    }
}

impl fmt::Debug for Seconds {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:.4}s", self.0)
    }
}
