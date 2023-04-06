use crate::state_monitor::{MonitoredValue, StateMonitor};
use std::time::Duration;

pub(crate) struct RepositoryStats {
    // This indicates how many requests for index nodes are currently in flight.  It is used by the
    // UI to indicate that the index is being synchronized.
    pub index_requests_inflight: MonitoredValue<u64>,
    pub block_requests_inflight: MonitoredValue<u64>,
    pub pending_requests: MonitoredValue<u64>,
    pub total_requests_cummulative: MonitoredValue<u64>,
    pub request_timeouts: MonitoredValue<u64>,

    pub request_queue_durations: DurationRanges,
    pub request_inflight_durations: DurationRanges,

    pub db_acquire_durations: DurationRanges,
    pub db_begin_read_durations: DurationRanges,
    pub db_begin_write_durations: DurationRanges,
}

impl RepositoryStats {
    pub fn new(monitor: StateMonitor) -> Self {
        Self {
            index_requests_inflight: monitor.make_value("index requests inflight", 0),
            block_requests_inflight: monitor.make_value("block requests inflight", 0),
            pending_requests: monitor.make_value("pending requests", 0),
            total_requests_cummulative: monitor.make_value("total requests cummulative", 0),
            request_timeouts: monitor.make_value("request timeouts", 0),
            request_queue_durations: DurationRanges::new(
                monitor.make_child("request queue durations"),
            ),
            request_inflight_durations: DurationRanges::new(
                monitor.make_child("request inflight durations"),
            ),

            db_acquire_durations: DurationRanges::new(monitor.make_child("db acquire durations")),
            db_begin_read_durations: DurationRanges::new(
                monitor.make_child("db begin read durations"),
            ),
            db_begin_write_durations: DurationRanges::new(
                monitor.make_child("db begin write durations"),
            ),
        }
    }
}

#[derive(Clone)]
pub(crate) struct DurationRanges {
    pub lt_00200: MonitoredValue<u64>,
    pub lt_00500: MonitoredValue<u64>,
    pub lt_01000: MonitoredValue<u64>,
    pub lt_03000: MonitoredValue<u64>,
    pub lt_10000: MonitoredValue<u64>,
    pub lt_30000: MonitoredValue<u64>,
    pub ge_30000: MonitoredValue<u64>,
}

impl DurationRanges {
    pub fn new(monitor: StateMonitor) -> Self {
        Self {
            // Make sure the labels are chosen such that when sorted lexicographically they are
            // also sorted numerically (from shortest to longest)
            lt_00200: monitor.make_value("<  0.2s", 0),
            lt_00500: monitor.make_value("<  0.5s", 0),
            lt_01000: monitor.make_value("<  1s", 0),
            lt_03000: monitor.make_value("<  3s", 0),
            lt_10000: monitor.make_value("< 10s", 0),
            lt_30000: monitor.make_value("< 30s", 0),
            ge_30000: monitor.make_value(">= 30s", 0),
        }
    }

    pub fn note(&self, duration: Duration) {
        let ms = duration.as_millis();

        if ms < 200 {
            *self.lt_00200.get() += 1;
        } else if ms < 500 {
            *self.lt_00500.get() += 1;
        } else if ms < 1000 {
            *self.lt_01000.get() += 1;
        } else if ms < 3000 {
            *self.lt_03000.get() += 1;
        } else if ms < 10000 {
            *self.lt_10000.get() += 1;
        } else if ms < 30000 {
            *self.lt_30000.get() += 1;
        } else {
            *self.ge_30000.get() += 1;
        }
    }
}
