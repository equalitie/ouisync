use btdht::InfoHash;
use metrics::{
    Counter, Gauge, Histogram, Key, KeyName, Level, Metadata, Recorder, SharedString, Unit,
};
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
    pub info_hash: MonitoredValue<Option<InfoHash>>,
    pub traffic: TrafficMonitor,
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
        let span = tracing::info_span!("repo", message = node.id().name());

        let info_hash = node.make_value("info-hash", None);
        let traffic = TrafficMonitor::new(recorder);
        let scan_job = JobMonitor::new(&node, recorder, "scan");
        let merge_job = JobMonitor::new(&node, recorder, "merge");
        let prune_job = JobMonitor::new(&node, recorder, "prune");
        let trash_job = JobMonitor::new(&node, recorder, "trash");

        Self {
            info_hash,
            traffic,
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

#[derive(Clone)]
pub(crate) struct TrafficMonitor {
    // Total number of index requests sent.
    pub index_requests_sent: Counter,
    // Current number of sent index request for which responses haven't been received yet.
    pub index_requests_inflight: Gauge,
    // Total number of block requests sent.
    pub block_requests_sent: Counter,
    // Current number of sent block request for which responses haven't been received yet.
    pub block_requests_inflight: Gauge,
    // Total number of received requests
    pub requests_received: Counter,
    // Time from sending a request to receiving its response.
    pub request_latency: Histogram,
    // Total number of timeouted requests.
    pub request_timeouts: Counter,

    // Total number of responses sent.
    pub responses_sent: Counter,
    // Total number of responses received.
    pub responses_received: Counter,
}

impl TrafficMonitor {
    pub fn new<R>(recorder: &R) -> Self
    where
        R: Recorder + ?Sized,
    {
        Self {
            index_requests_sent: create_counter(recorder, "index requests sent", Unit::Count),
            index_requests_inflight: create_gauge(recorder, "index requests inflight", Unit::Count),
            block_requests_sent: create_counter(recorder, "block requests sent", Unit::Count),
            block_requests_inflight: create_gauge(recorder, "block requests inflight", Unit::Count),
            requests_received: create_counter(recorder, "requests received", Unit::Count),
            request_latency: create_histogram(recorder, "request latency", Unit::Seconds),
            request_timeouts: create_counter(recorder, "request timeouts", Unit::Count),
            responses_sent: create_counter(recorder, "responses sent", Unit::Count),
            responses_received: create_counter(recorder, "responses received", Unit::Count),
        }
    }

    pub fn record(&self, event: RequestEvent, kind: RequestKind) {
        match (event, kind) {
            (RequestEvent::Send, RequestKind::Index) => {
                self.index_requests_sent.increment(1);
                self.index_requests_inflight.increment(1.0);
            }
            (RequestEvent::Send, RequestKind::Block) => {
                self.block_requests_sent.increment(1);
                self.block_requests_inflight.increment(1.0);
            }
            (
                RequestEvent::Success { .. }
                | RequestEvent::Failure { .. }
                | RequestEvent::Timeout
                | RequestEvent::Cancel,
                RequestKind::Index,
            ) => {
                self.index_requests_inflight.decrement(1.0);
            }
            (
                RequestEvent::Success { .. }
                | RequestEvent::Failure { .. }
                | RequestEvent::Timeout
                | RequestEvent::Cancel,
                RequestKind::Block,
            ) => {
                self.block_requests_inflight.decrement(1.0);
            }
            (
                RequestEvent::Send
                | RequestEvent::Success { .. }
                | RequestEvent::Failure { .. }
                | RequestEvent::Timeout
                | RequestEvent::Cancel,
                RequestKind::Other,
            ) => (),
        }

        match event {
            RequestEvent::Success { rtt } | RequestEvent::Failure { rtt } => {
                self.request_latency.record(rtt);
            }
            RequestEvent::Timeout => {
                self.request_timeouts.increment(1);
            }
            RequestEvent::Send | RequestEvent::Cancel => (),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) enum RequestKind {
    Index,
    Block,
    Other,
}

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) enum RequestEvent {
    Send,
    Success { rtt: Duration },
    Failure { rtt: Duration },
    Timeout,
    Cancel,
}

pub(crate) struct JobMonitor {
    name: String,
    count_running_tx: watch::Sender<usize>,
    count_total: AtomicU64,
    time: Histogram,
}

impl JobMonitor {
    fn new<R>(parent_node: &StateMonitor, recorder: &R, name: &str) -> Self
    where
        R: Recorder + ?Sized,
    {
        let time = create_histogram(recorder, format!("{name} time"), Unit::Seconds);
        let state = parent_node.make_value(format!("{name} state"), JobState::Idle);

        Self::from_parts(name, time, state)
    }

    fn from_parts(name: &str, time: Histogram, state: MonitoredValue<JobState>) -> Self {
        let (count_running_tx, mut count_running_rx) = watch::channel(0);

        task::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            let mut start = None;

            loop {
                select! {
                    result = count_running_rx.changed() => {
                        if result.is_err() {
                            *state.get() = JobState::Idle;
                            break;
                        }

                        match (start, *count_running_rx.borrow()) {
                            (Some(_), 0) => {
                                start = None;
                                *state.get() = JobState::Idle;
                            }
                            (None, 1) => {
                                start = Some(Instant::now());
                            }
                            (Some(_) | None, _) => (),
                        }
                    }
                    _ = interval.tick(), if start.is_some() => {
                        *state.get() = JobState::Running(start.unwrap().elapsed());
                    }
                }
            }
        });

        Self {
            name: name.to_string(),
            count_running_tx,
            count_total: AtomicU64::new(0),
            time,
        }
    }

    /// Runs a monitored job.
    ///
    /// A single `JobMonitor` can monitor multiple concurrent jobs but they are threated as a single
    /// unit - the monitoring starts when the first job starts and stops when the last job stops.
    pub(crate) async fn run<F, E>(&self, f: F) -> bool
    where
        F: Future<Output = Result<(), E>>,
        E: fmt::Debug,
    {
        async move {
            let guard = JobGuard::start(self);
            let start = Instant::now();

            let result = f.await;
            let is_ok = result.is_ok();

            self.time.record(start.elapsed());

            guard.complete(result);

            is_ok
        }
        .instrument(tracing::info_span!(
            "job",
            message = self.name,
            id = self.count_total.fetch_add(1, Ordering::Relaxed),
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

        monitor.count_running_tx.send_modify(|count| *count += 1);

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

        self.monitor
            .count_running_tx
            .send_modify(|count| *count -= 1);
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

fn create_counter<R: Recorder + ?Sized, N: Into<SharedString>>(
    recorder: &R,
    name: N,
    unit: Unit,
) -> Counter {
    let name = KeyName::from(name);
    recorder.describe_counter(name.clone(), Some(unit), "".into());
    recorder.register_counter(
        &Key::from_name(name),
        &Metadata::new(module_path!(), Level::INFO, None),
    )
}

fn create_gauge<R: Recorder + ?Sized, N: Into<SharedString>>(
    recorder: &R,
    name: N,
    unit: Unit,
) -> Gauge {
    let name = KeyName::from(name);
    recorder.describe_gauge(name.clone(), Some(unit), "".into());
    recorder.register_gauge(
        &Key::from_name(name),
        &Metadata::new(module_path!(), Level::INFO, None),
    )
}

fn create_histogram<R: Recorder + ?Sized, N: Into<SharedString>>(
    recorder: &R,
    name: N,
    unit: Unit,
) -> Histogram {
    let name = KeyName::from(name);
    recorder.describe_histogram(name.clone(), Some(unit), "".into());
    recorder.register_histogram(
        &Key::from_name(name),
        &Metadata::new(module_path!(), Level::INFO, None),
    )
}
