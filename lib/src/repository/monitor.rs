use crate::{
    collections::HashMap,
    metrics::{Metric, MetricName, Metrics, Report, ReportItem},
    state_monitor::{MonitoredValue, StateMonitor},
};
use btdht::InfoHash;
use scoped_task::ScopedJoinHandle;
use std::{
    fmt,
    future::Future,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use tokio::{
    select,
    sync::{oneshot, watch},
    task,
    time::{self, MissedTickBehavior},
};
use tracing::{Instrument, Span};

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
    pub handle_request_metric: Metric,

    pub scan_job: JobMonitor,
    pub merge_job: JobMonitor,
    pub prune_job: JobMonitor,
    pub trash_job: JobMonitor,

    span: Span,
    node: StateMonitor,
    _report_metrics_task: ScopedJoinHandle<()>,
}

impl RepositoryMonitor {
    pub fn new(parent: StateMonitor, metrics: Metrics, name: &str) -> Self {
        let span = tracing::info_span!("repo", name);
        let node = parent.make_child(name);

        let index_requests_inflight = node.make_value("index requests inflight", 0);
        let block_requests_inflight = node.make_value("block requests inflight", 0);
        let pending_requests = node.make_value("pending requests", 0);
        let total_requests_cummulative = node.make_value("total requests cummulative", 0);
        let request_timeouts = node.make_value("request timeouts", 0);
        let info_hash = node.make_value("info-hash", None);

        let handle_response_metric = metrics.get("handle_response");
        let handle_root_node_metric = metrics.get("handle_root_node");
        let handle_inner_nodes_metric = metrics.get("handle_inner_node");
        let handle_leaf_nodes_metric = metrics.get("handle_leaf_node");
        let handle_block_metric = metrics.get("handle_block");
        let request_queued_metric = metrics.get("request queued");
        let request_inflight_metric = metrics.get("request inflight");
        let handle_request_metric = metrics.get("handle_request");

        let scan_job = JobMonitor::new(&node, &metrics, "scan".into());
        let merge_job = JobMonitor::new(&node, &metrics, "merge".into());
        let prune_job = JobMonitor::new(&node, &metrics, "prune".into());
        let trash_job = JobMonitor::new(&node, &metrics, "trash".into());

        let report_metrics_task = scoped_task::spawn(report_metrics(metrics, node.clone()));

        Self {
            index_requests_inflight,
            block_requests_inflight,
            pending_requests,
            total_requests_cummulative,
            request_timeouts,
            info_hash,

            handle_response_metric,
            handle_root_node_metric,
            handle_inner_nodes_metric,
            handle_leaf_nodes_metric,
            handle_block_metric,
            request_queued_metric,
            request_inflight_metric,
            handle_request_metric,

            scan_job,
            merge_job,
            prune_job,
            trash_job,

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

    time_last: MonitoredValue<Format<Duration>>,
    time_min: MonitoredValue<Format<Duration>>,
    time_max: MonitoredValue<Format<Duration>>,
    time_mean: MonitoredValue<Format<Duration>>,
    time_stdev: MonitoredValue<Format<Duration>>,
    time_p50: MonitoredValue<Format<Duration>>,
    time_p90: MonitoredValue<Format<Duration>>,
    time_p99: MonitoredValue<Format<Duration>>,
    time_p999: MonitoredValue<Format<Duration>>,

    throughput_last: MonitoredValue<Format<f64>>,
    throughput_min: MonitoredValue<Format<f64>>,
    throughput_max: MonitoredValue<Format<f64>>,
    throughput_mean: MonitoredValue<Format<f64>>,
    throughput_stdev: MonitoredValue<Format<f64>>,
    throughput_p50: MonitoredValue<Format<f64>>,
    throughput_p90: MonitoredValue<Format<f64>>,
    throughput_p99: MonitoredValue<Format<f64>>,
    throughput_p999: MonitoredValue<Format<f64>>,
}

impl MetricMonitor {
    fn new(node: StateMonitor) -> Self {
        let time = node.make_child("time stats");
        let throughput = node.make_child("throughput stats");

        Self {
            count: node.make_value("count", 0),

            time_last: node.make_value("time", Format(Duration::ZERO)),
            time_min: time.make_value("min", Format(Duration::ZERO)),
            time_max: time.make_value("max", Format(Duration::ZERO)),
            time_mean: time.make_value("mean", Format(Duration::ZERO)),
            time_stdev: time.make_value("stdev", Format(Duration::ZERO)),
            time_p50: time.make_value("50%", Format(Duration::ZERO)),
            time_p90: time.make_value("90%", Format(Duration::ZERO)),
            time_p99: time.make_value("99%", Format(Duration::ZERO)),
            time_p999: time.make_value("99.9%", Format(Duration::ZERO)),

            throughput_last: node.make_value("throughput", Format(0.0)),
            throughput_min: throughput.make_value("min", Format(0.0)),
            throughput_max: throughput.make_value("max", Format(0.0)),
            throughput_mean: throughput.make_value("mean", Format(0.0)),
            throughput_stdev: throughput.make_value("stdev", Format(0.0)),
            throughput_p50: throughput.make_value("50%", Format(0.0)),
            throughput_p90: throughput.make_value("90%", Format(0.0)),
            throughput_p99: throughput.make_value("99%", Format(0.0)),
            throughput_p999: throughput.make_value("99.9%", Format(0.0)),
        }
    }

    fn update(&self, item: ReportItem<'_>) {
        *self.count.get() = item.time.count();

        *self.time_last.get() = Format(item.time.last());
        *self.time_min.get() = Format(item.time.min());
        *self.time_max.get() = Format(item.time.max());
        *self.time_mean.get() = Format(item.time.mean());
        *self.time_stdev.get() = Format(item.time.stdev());
        *self.time_p50.get() = Format(item.time.value_at_quantile(0.5));
        *self.time_p90.get() = Format(item.time.value_at_quantile(0.9));
        *self.time_p99.get() = Format(item.time.value_at_quantile(0.99));
        *self.time_p999.get() = Format(item.time.value_at_quantile(0.999));

        *self.throughput_last.get() = Format(item.throughput.last());
        *self.throughput_min.get() = Format(item.throughput.min());
        *self.throughput_max.get() = Format(item.throughput.max());
        *self.throughput_mean.get() = Format(item.throughput.mean());
        *self.throughput_stdev.get() = Format(item.throughput.stdev());
        *self.throughput_p50.get() = Format(item.throughput.value_at_quantile(0.5));
        *self.throughput_p90.get() = Format(item.throughput.value_at_quantile(0.9));
        *self.throughput_p99.get() = Format(item.throughput.value_at_quantile(0.99));
        *self.throughput_p999.get() = Format(item.throughput.value_at_quantile(0.999));
    }
}

struct Format<T>(T);

impl fmt::Debug for Format<Duration> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:.4}s", self.0.as_secs_f64())
    }
}

impl fmt::Debug for Format<f64> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:.1}", self.0)
    }
}

pub(crate) struct JobMonitor {
    tx: watch::Sender<bool>,
    metric: Metric,
    counter: AtomicU64,
}

impl JobMonitor {
    fn new(parent_node: &StateMonitor, metrics: &Metrics, name: MetricName) -> Self {
        let value = parent_node.make_value(format!("{name} state"), JobState::Idle);
        let metric = metrics.get(name);

        let (tx, mut rx) = watch::channel(false);

        task::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            let mut start = None;

            loop {
                select! {
                    result = rx.changed() => {
                        if result.is_err() {
                            *value.get() = JobState::Idle;
                            break;
                        }

                        if *rx.borrow() {
                            start = Some(Instant::now());
                        } else {
                            start = None;
                            *value.get() = JobState::Idle;
                        }
                    }
                    _ = interval.tick(), if start.is_some() => {
                        *value.get() = JobState::Running(start.unwrap().elapsed());
                    }
                }
            }
        });

        Self {
            tx,
            metric,
            counter: AtomicU64::new(0),
        }
    }

    pub(crate) async fn run<F, E>(&self, f: F)
    where
        F: Future<Output = Result<(), E>>,
        E: fmt::Debug,
    {
        if self.tx.send_replace(true) {
            panic!("job monitor can monitor at most one job at a time");
        }

        async move {
            let guard = JobGuard::start(self);
            let _timing = self.metric.start();

            let result = f.await;

            guard.complete(result);
        }
        .instrument(tracing::info_span!(
            "job",
            name = self.metric.name().as_ref(),
            id = self.counter.fetch_add(1, Ordering::Relaxed),
        ))
        .await
    }
}

pub(crate) struct JobGuard<'a> {
    monitor: &'a JobMonitor,
    span: Span,
    completed: bool,
}

impl<'a> JobGuard<'a> {
    fn start(monitor: &'a JobMonitor) -> Self {
        let span = Span::current();

        tracing::trace!(parent: &span, "job started");

        Self {
            monitor,
            span,
            completed: false,
        }
    }

    fn complete<E: fmt::Debug>(mut self, result: Result<(), E>) {
        self.completed = true;
        tracing::trace!(parent: &self.span, ?result, "job completed");
    }
}

impl Drop for JobGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            tracing::trace!(parent: &self.span, "job interrupted");
        }

        self.monitor.tx.send(false).ok();
    }
}

enum JobState {
    Idle,
    Running(Duration),
}

impl fmt::Debug for JobState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Running(duration) => write!(f, "running for {:.1}s", duration.as_secs_f64()),
        }
    }
}
