use btdht::InfoHash;
use metrics::{Gauge, Histogram, Key, KeyName, Level, Metadata, Recorder, Unit};
use state_monitor::{MonitoredValue, StateMonitor};
use std::{
    fmt,
    future::Future,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use tokio::{
    select,
    sync::watch,
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

    pub handle_response_metric: TaskMetrics,
    pub handle_root_node_metric: TaskMetrics,
    pub handle_inner_nodes_metric: TaskMetrics,
    pub handle_leaf_nodes_metric: TaskMetrics,
    pub handle_block_metric: TaskMetrics,
    pub handle_block_not_found_metric: TaskMetrics,
    pub request_queued_metric: TaskMetrics,
    pub request_inflight_metric: TaskMetrics,
    pub handle_request_metric: TaskMetrics,

    pub scan_job: JobMonitor,
    pub merge_job: JobMonitor,
    pub prune_job: JobMonitor,
    pub trash_job: JobMonitor,

    span: Span,
    node: StateMonitor,
}

impl RepositoryMonitor {
    pub fn new<R>(node: StateMonitor, recorder: &R) -> Self
    where
        R: Recorder + ?Sized,
    {
        let span = tracing::info_span!("repo", repo = node.id().name());

        // to expose the repo name as metrics label (via metrics-tracing-context)
        let span_enter = span.enter();

        let index_requests_inflight = node.make_value("index requests inflight", 0);
        let block_requests_inflight = node.make_value("block requests inflight", 0);
        let pending_requests = node.make_value("pending requests", 0);
        let total_requests_cummulative = node.make_value("total requests cummulative", 0);
        let request_timeouts = node.make_value("request timeouts", 0);
        let info_hash = node.make_value("info-hash", None);

        let handle_response_metric = TaskMetrics::new(recorder, "handle_response");
        let handle_root_node_metric = TaskMetrics::new(recorder, "handle_root_node");
        let handle_inner_nodes_metric = TaskMetrics::new(recorder, "handle_inner_node");
        let handle_leaf_nodes_metric = TaskMetrics::new(recorder, "handle_leaf_node");
        let handle_block_metric = TaskMetrics::new(recorder, "handle_block");
        let handle_block_not_found_metric = TaskMetrics::new(recorder, "handle_block_not_found");
        let request_queued_metric = TaskMetrics::new(recorder, "request queued");
        let request_inflight_metric = TaskMetrics::new(recorder, "request inflight");
        let handle_request_metric = TaskMetrics::new(recorder, "handle_request");

        let scan_job = JobMonitor::new(&node, recorder, "scan");
        let merge_job = JobMonitor::new(&node, recorder, "merge");
        let prune_job = JobMonitor::new(&node, recorder, "prune");
        let trash_job = JobMonitor::new(&node, recorder, "trash");

        drop(span_enter);

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
            handle_block_not_found_metric,
            request_queued_metric,
            request_inflight_metric,
            handle_request_metric,

            scan_job,
            merge_job,
            prune_job,
            trash_job,

            span,
            node,
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

pub(crate) struct TaskMetrics {
    time_last: Gauge,
    time_histogram: Histogram,
    // TODO: throughtput
}

impl TaskMetrics {
    fn new<R>(recorder: &R, name: &str) -> Self
    where
        R: Recorder + ?Sized,
    {
        let meta = Metadata::new(module_path!(), Level::INFO, None);

        let key_name = KeyName::from(format!("{name}/time/last"));
        recorder.describe_gauge(key_name.clone(), Some(Unit::Seconds), Default::default());
        let time_last = recorder.register_gauge(&Key::from(key_name), &meta);

        let key_name = KeyName::from(format!("{name}/time"));
        recorder.describe_histogram(key_name.clone(), Some(Unit::Seconds), Default::default());
        let time_histogram = recorder.register_histogram(&Key::from(key_name), &meta);

        Self {
            time_last,
            time_histogram,
        }
    }

    pub(crate) fn record(&self, value: Duration) {
        let secs = value.as_secs_f64();
        self.time_last.set(secs);
        self.time_histogram.record(secs);
    }

    pub(crate) async fn measure_ok<F, T, E>(&self, fut: F) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
    {
        let start = Instant::now();
        let result = fut.await;

        if result.is_ok() {
            self.record(start.elapsed());
        }

        result
    }
}

pub(crate) struct JobMonitor {
    tx: watch::Sender<bool>,
    metrics: TaskMetrics,
    counter: AtomicU64,
    name: String,
}

impl JobMonitor {
    fn new<R>(parent_node: &StateMonitor, recorder: &R, name: &str) -> Self
    where
        R: Recorder + ?Sized,
    {
        let value = parent_node.make_value(format!("{name} state"), JobState::Idle);
        let metrics = TaskMetrics::new(recorder, name);

        Self::from_parts(value, metrics, name)
    }

    fn from_parts(value: MonitoredValue<JobState>, metrics: TaskMetrics, name: &str) -> Self {
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
            metrics,
            counter: AtomicU64::new(0),
            name: name.to_string(),
        }
    }

    pub(crate) async fn run<F, E>(&self, f: F) -> bool
    where
        F: Future<Output = Result<(), E>>,
        E: fmt::Debug,
    {
        if self.tx.send_replace(true) {
            panic!("job monitor can monitor at most one job at a time");
        }

        async move {
            let guard = JobGuard::start(self);
            let start = Instant::now();

            let result = f.await;
            let is_ok = result.is_ok();

            self.metrics.record(start.elapsed());

            guard.complete(result);

            is_ok
        }
        .instrument(tracing::info_span!(
            "job",
            message = self.name,
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

        tracing::trace!(parent: &span, "Job started");

        Self {
            monitor,
            span,
            completed: false,
        }
    }

    fn complete<E: fmt::Debug>(mut self, result: Result<(), E>) {
        self.completed = true;
        tracing::trace!(parent: &self.span, ?result, "Job completed");
    }
}

impl Drop for JobGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            tracing::trace!(parent: &self.span, "Job interrupted");
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
