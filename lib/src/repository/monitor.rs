use crate::{
    state_monitor::{MonitoredValue, StateMonitor},
    timing::{Clock, Clocks},
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
    pub info_hash: MonitoredValue<Option<InfoHash>>,

    pub clock_handle_root_node: Clock,
    pub clock_handle_inner_nodes: Clock,
    pub clock_handle_leaf_nodes: Clock,
    pub clock_handle_block: Clock,
    pub clock_request_queued: Clock,
    pub clock_request_inflight: Clock,

    span: Span,
    node: StateMonitor,
    _clocks: Clocks,
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
            _clocks: clocks,
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
