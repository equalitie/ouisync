use crate::state_monitor::{DurationRanges, StateMonitor};

pub(super) struct DatabaseMonitor {
    pub acquire_durations: DurationRanges,
    pub begin_read_durations: DurationRanges,
    pub begin_write_durations: DurationRanges,
}

impl DatabaseMonitor {
    pub fn new(parent: &StateMonitor) -> Self {
        Self {
            acquire_durations: DurationRanges::new(parent.make_child("db acquire durations")),
            begin_read_durations: DurationRanges::new(parent.make_child("db begin read durations")),
            begin_write_durations: DurationRanges::new(
                parent.make_child("db begin write durations"),
            ),
        }
    }
}
