use std::{
    ops::{Deref, DerefMut},
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::Span;

pub(crate) struct RepositoryStats {
    values: Arc<Mutex<Values>>,
    span: Span,
}

impl RepositoryStats {
    pub fn new(span: Span) -> Self {
        let values = Arc::new(Mutex::new(Values::default()));

        Self { values, span }
    }

    pub fn write(&self) -> Writer {
        let new = *self.values.lock().unwrap();
        let old = &self.values;

        Writer {
            old,
            new,
            span: &self.span,
        }
    }
}

pub(crate) struct Writer<'a> {
    old: &'a Mutex<Values>,
    new: Values,
    span: &'a Span,
}

impl<'a> Deref for Writer<'a> {
    type Target = Values;

    fn deref(&self) -> &Self::Target {
        &self.new
    }
}

impl<'a> DerefMut for Writer<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.new
    }
}

impl<'a> Drop for Writer<'a> {
    fn drop(&mut self) {
        let mut old = self.old.lock().unwrap();

        if self.new.index_requests_inflight != old.index_requests_inflight {
            state_monitor!(parent: self.span, index_requests_inflight = self.new.index_requests_inflight);
        }

        if self.new.block_requests_inflight != old.block_requests_inflight {
            state_monitor!(parent: self.span, block_requests_inflight = self.new.block_requests_inflight);
        }

        if self.new.total_requests_cummulative != old.total_requests_cummulative {
            state_monitor!(parent: self.span, total_requests_cummulative = self.new.total_requests_cummulative);
        }

        if self.new.request_timeouts != old.request_timeouts {
            state_monitor!(parent: self.span, request_timeouts = self.new.request_timeouts);
        }

        if self.new.db_acquire_durations != old.db_acquire_durations {
            let ds = self.new.db_acquire_durations;
            state_monitor!(parent: self.span, db_acquire_0_below_200ms = ds.below_200ms);
            state_monitor!(parent: self.span, db_acquire_1_from_200ms_to_500ms = ds.from_200ms_to_500ms);
            state_monitor!(parent: self.span, db_acquire_2_from_500ms_to_1s = ds.from_500ms_to_1s);
            state_monitor!(parent: self.span, db_acquire_3_from_1s_to_3s = ds.from_1s_to_3s);
            state_monitor!(parent: self.span, db_acquire_4_from_3s_to_10s = ds.from_3s_to_10s);
            state_monitor!(parent: self.span, db_acquire_5_from_10s_to_30s = ds.from_10s_to_30s);
            state_monitor!(parent: self.span, db_acquire_6_more_than_30s = ds.more_than_30s);
        }

        if self.new.db_read_begin_durations != old.db_read_begin_durations {
            let ds = self.new.db_read_begin_durations;
            state_monitor!(parent: self.span, db_read_begin_0_below_200ms = ds.below_200ms);
            state_monitor!(parent: self.span, db_read_begin_1_from_200ms_to_500ms = ds.from_200ms_to_500ms);
            state_monitor!(parent: self.span, db_read_begin_2_from_500ms_to_1s = ds.from_500ms_to_1s);
            state_monitor!(parent: self.span, db_read_begin_3_from_1s_to_3s = ds.from_1s_to_3s);
            state_monitor!(parent: self.span, db_read_begin_4_from_3s_to_10s = ds.from_3s_to_10s);
            state_monitor!(parent: self.span, db_read_begin_5_from_10s_to_30s = ds.from_10s_to_30s);
            state_monitor!(parent: self.span, db_read_begin_6_more_than_30s = ds.more_than_30s);
        }

        if self.new.db_write_begin_durations != old.db_write_begin_durations {
            let ds = self.new.db_write_begin_durations;
            state_monitor!(parent: self.span, db_write_begin_0_below_200ms = ds.below_200ms);
            state_monitor!(parent: self.span, db_write_begin_1_from_200ms_to_500ms = ds.from_200ms_to_500ms);
            state_monitor!(parent: self.span, db_write_begin_2_from_500ms_to_1s = ds.from_500ms_to_1s);
            state_monitor!(parent: self.span, db_write_begin_3_from_1s_to_3s = ds.from_1s_to_3s);
            state_monitor!(parent: self.span, db_write_begin_4_from_3s_to_10s = ds.from_3s_to_10s);
            state_monitor!(parent: self.span, db_write_begin_5_from_10s_to_30s = ds.from_10s_to_30s);
            state_monitor!(parent: self.span, db_write_begin_6_more_than_30s = ds.more_than_30s);
        }

        *old = self.new;
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Default, Debug)]
struct DurationRanges {
    pub below_200ms: u64,
    pub from_200ms_to_500ms: u64,
    pub from_500ms_to_1s: u64,
    pub from_1s_to_3s: u64,
    pub from_3s_to_10s: u64,
    pub from_10s_to_30s: u64,
    pub more_than_30s: u64,
}

impl DurationRanges {
    fn note_duration(&mut self, duration: Duration) {
        let ms = duration.as_millis();

        if ms < 200 {
            self.below_200ms += 1;
        } else if ms < 500 {
            self.from_200ms_to_500ms += 1;
        } else if ms < 1000 {
            self.from_500ms_to_1s += 1;
        } else if ms < 3000 {
            self.from_1s_to_3s += 1;
        } else if ms < 10000 {
            self.from_3s_to_10s += 1;
        } else if ms < 30000 {
            self.from_10s_to_30s += 1;
        } else {
            self.more_than_30s += 1;
        }
    }
}

#[derive(Copy, Clone, Default, Debug)]
pub(crate) struct Values {
    // This indicates how many requests for index nodes are currently in flight.  It is used by the
    // UI to indicate that the index is being synchronized.
    pub index_requests_inflight: u64,
    pub block_requests_inflight: u64,
    pub total_requests_cummulative: u64,
    pub request_timeouts: u64,

    db_acquire_durations: DurationRanges,
    db_read_begin_durations: DurationRanges,
    db_write_begin_durations: DurationRanges,
}

impl Values {
    pub(crate) fn note_db_acquire_duration(&mut self, duration: Duration) {
        self.db_acquire_durations.note_duration(duration);
    }

    pub(crate) fn note_db_read_begin_duration(&mut self, duration: Duration) {
        self.db_read_begin_durations.note_duration(duration);
    }

    pub(crate) fn note_db_write_begin_duration(&mut self, duration: Duration) {
        self.db_write_begin_durations.note_duration(duration);
    }
}
