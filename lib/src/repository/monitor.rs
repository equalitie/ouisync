use std::{fmt, time::Duration};

use crate::{
    collections::HashMap,
    metrics::{Metric, Metrics, Report, ReportItem},
    state_monitor::{MonitoredValue, StateMonitor},
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

    pub handle_response_metric: Metric,
    pub handle_root_node_metric: Metric,
    pub handle_inner_nodes_metric: Metric,
    pub handle_leaf_nodes_metric: Metric,
    pub handle_block_metric: Metric,
    pub request_queued_metric: Metric,
    pub request_inflight_metric: Metric,

    span: Span,
    node: StateMonitor,
    _report_metrics_task: ScopedJoinHandle<()>,
}

impl RepositoryMonitor {
    pub fn new(parent: StateMonitor, metrics: Metrics, name: &str) -> Self {
        let span = tracing::info_span!("repo", name);
        let node = parent.make_child(name);

        let handle_response_metric = metrics.get("handle_response");
        let handle_root_node_metric = metrics.get("handle_root_node");
        let handle_inner_nodes_metric = metrics.get("handle_inner_node");
        let handle_leaf_nodes_metric = metrics.get("handle_leaf_node");
        let handle_block_metric = metrics.get("handle_block");
        let request_queued_metric = metrics.get("request queued");
        let request_inflight_metric = metrics.get("request inflight");

        let report_metrics_task = scoped_task::spawn(report_metrics(metrics, node.clone()));

        Self {
            index_requests_inflight: node.make_value("index requests inflight", 0),
            block_requests_inflight: node.make_value("block requests inflight", 0),
            pending_requests: node.make_value("pending requests", 0),
            total_requests_cummulative: node.make_value("total requests cummulative", 0),
            request_timeouts: node.make_value("request timeouts", 0),
            info_hash: node.make_value("info-hash", None),

            handle_response_metric,
            handle_root_node_metric,
            handle_inner_nodes_metric,
            handle_leaf_nodes_metric,
            handle_block_metric,
            request_queued_metric,
            request_inflight_metric,

            span,
            node,
            _report_metrics_task: report_metrics_task,
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

async fn report_metrics(metrics: Metrics, monitor: StateMonitor) {
    let mut interval = time::interval(Duration::from_secs(1));
    let mut monitors = HashMap::new();

    loop {
        interval.tick().await;

        let (tx, rx) = oneshot::channel();
        let monitor = monitor.clone();

        metrics.report(move |report: &Report| {
            for item in report.items() {
                monitors
                    .entry(item.name.clone())
                    .or_insert_with(|| MetricMonitor::new(monitor.make_child(item.name.as_ref())))
                    .update(item);
            }

            tx.send(monitors).ok();
        });

        monitors = rx.await.unwrap();
    }
}

struct MetricMonitor {
    count: MonitoredValue<u64>,

    time_recent: MonitoredValue<Seconds>,
    time_min: MonitoredValue<Seconds>,
    time_max: MonitoredValue<Seconds>,
    time_mean: MonitoredValue<Seconds>,
    time_stdev: MonitoredValue<Seconds>,
    time_p50: MonitoredValue<Seconds>,
    time_p90: MonitoredValue<Seconds>,
    time_p99: MonitoredValue<Seconds>,
    time_p999: MonitoredValue<Seconds>,

    throughput_recent: MonitoredValue<Float>,
    throughput_min: MonitoredValue<u64>,
    throughput_max: MonitoredValue<u64>,
    throughput_mean: MonitoredValue<Float>,
    throughput_stdev: MonitoredValue<Float>,
    throughput_p50: MonitoredValue<u64>,
    throughput_p90: MonitoredValue<u64>,
    throughput_p99: MonitoredValue<u64>,
    throughput_p999: MonitoredValue<u64>,
}

impl MetricMonitor {
    fn new(node: StateMonitor) -> Self {
        let time = node.make_child("time");
        let throughput = node.make_child("throughput");

        Self {
            count: node.make_value("count", 0),

            time_recent: time.make_value("recent", Seconds(0.0)),
            time_min: time.make_value("min", Seconds(0.0)),
            time_max: time.make_value("max", Seconds(0.0)),
            time_mean: time.make_value("mean", Seconds(0.0)),
            time_stdev: time.make_value("stdev", Seconds(0.0)),
            time_p50: time.make_value("50%", Seconds(0.0)),
            time_p90: time.make_value("90%", Seconds(0.0)),
            time_p99: time.make_value("99%", Seconds(0.0)),
            time_p999: time.make_value("99.9%", Seconds(0.0)),

            throughput_recent: throughput.make_value("recent", Float(0.0)),
            throughput_min: throughput.make_value("min", 0),
            throughput_max: throughput.make_value("max", 0),
            throughput_mean: throughput.make_value("mean", Float(0.0)),
            throughput_stdev: throughput.make_value("stdev", Float(0.0)),
            throughput_p50: throughput.make_value("50%", 0),
            throughput_p90: throughput.make_value("90%", 0),
            throughput_p99: throughput.make_value("99%", 0),
            throughput_p999: throughput.make_value("99.9%", 0),
        }
    }

    fn update(&self, item: ReportItem<'_>) {
        *self.count.get() = item.time_histogram.len();

        *self.time_recent.get() = Seconds::from_ns_f(item.time_recent);
        *self.time_min.get() = Seconds::from_ns(item.time_histogram.min());
        *self.time_max.get() = Seconds::from_ns(item.time_histogram.max());
        *self.time_mean.get() = Seconds::from_ns_f(item.time_histogram.mean());
        *self.time_stdev.get() = Seconds::from_ns_f(item.time_histogram.stdev());
        *self.time_p50.get() = Seconds::from_ns(item.time_histogram.value_at_quantile(0.5));
        *self.time_p90.get() = Seconds::from_ns(item.time_histogram.value_at_quantile(0.9));
        *self.time_p99.get() = Seconds::from_ns(item.time_histogram.value_at_quantile(0.99));
        *self.time_p999.get() = Seconds::from_ns(item.time_histogram.value_at_quantile(0.999));

        *self.throughput_recent.get() = Float(item.throughput_recent);
        *self.throughput_min.get() = item.throughput_histogram.min();
        *self.throughput_max.get() = item.throughput_histogram.max();
        *self.throughput_mean.get() = Float(item.throughput_histogram.mean());
        *self.throughput_stdev.get() = Float(item.throughput_histogram.stdev());
        *self.throughput_p50.get() = item.throughput_histogram.value_at_quantile(0.5);
        *self.throughput_p90.get() = item.throughput_histogram.value_at_quantile(0.9);
        *self.throughput_p99.get() = item.throughput_histogram.value_at_quantile(0.99);
        *self.throughput_p999.get() = item.throughput_histogram.value_at_quantile(0.999);
    }
}

struct Seconds(f64);

impl Seconds {
    fn from_ns(ns: u64) -> Self {
        Self::from_ns_f(ns as f64)
    }

    fn from_ns_f(ns: f64) -> Self {
        Self(ns / 1_000_000_000.0)
    }
}

impl fmt::Debug for Seconds {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:.4}s", self.0)
    }
}

struct Float(f64);

impl fmt::Debug for Float {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:.1}", self.0)
    }
}
