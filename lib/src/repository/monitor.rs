use crate::{
    state_monitor::{DurationRanges, MonitoredValue, StateMonitor},
    timing,
};
use btdht::InfoHash;
use tracing::Span;

pub(crate) struct RepositoryMonitor {
    // This indicates how many requests for index nodes are currently in flight.  It is used by the
    // UI to indicate that the index is being synchronized.
    pub index_requests_inflight: MonitoredValue<u64>,
    pub block_requests_inflight: MonitoredValue<u64>,
    pub pending_requests: MonitoredValue<u64>,
    pub total_requests_cummulative: MonitoredValue<u64>,
    pub request_timeouts: MonitoredValue<u64>,
    pub request_queue_durations: DurationRanges,
    pub request_inflight_durations: DurationRanges,
    pub info_hash: MonitoredValue<Option<InfoHash>>,
    span: Span,
    node: StateMonitor,
    timer: timing::Timer,
}

impl RepositoryMonitor {
    pub fn new(parent: StateMonitor, timer: timing::Timer, name: &str) -> Self {
        let span = tracing::info_span!("repo", name);
        let node = parent.make_child(name);

        Self {
            index_requests_inflight: node.make_value("index requests inflight", 0),
            block_requests_inflight: node.make_value("block requests inflight", 0),
            pending_requests: node.make_value("pending requests", 0),
            total_requests_cummulative: node.make_value("total requests cummulative", 0),
            request_timeouts: node.make_value("request timeouts", 0),
            request_queue_durations: DurationRanges::new(
                node.make_child("request queue durations"),
            ),
            request_inflight_durations: DurationRanges::new(
                node.make_child("request inflight durations"),
            ),
            info_hash: node.make_value("info-hash", None),
            span,
            node,
            timer,
        }
    }

    pub fn span(&self) -> &Span {
        &self.span
    }

    pub fn node(&self) -> &StateMonitor {
        &self.node
    }

    pub fn timer(&self) -> &timing::Timer {
        &self.timer
    }

    pub fn name(&self) -> &str {
        self.node.id().name()
    }
}
