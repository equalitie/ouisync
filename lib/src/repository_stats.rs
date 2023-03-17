use std::{
    ops::{Deref, DerefMut},
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::Span;

pub(crate) struct RepositoryStats {
    values: Arc<Mutex<Values>>,
    span: Span,
    db_acquire: Span,
    db_read_begin: Span,
    db_write_begin: Span,
}

impl RepositoryStats {
    pub fn new(span: Span) -> Self {
        let values = Arc::new(Mutex::new(Values::default()));

        let db_acquire = tracing::info_span!(parent: span.clone(), "db_acquire");
        let db_read_begin = tracing::info_span!(parent: span.clone(), "db_read_begin");
        let db_write_begin = tracing::info_span!(parent: span.clone(), "db_write_begin");

        Self {
            values,
            span,
            db_acquire,
            db_read_begin,
            db_write_begin,
        }
    }

    pub fn write(&self) -> Writer {
        let new = *self.values.lock().unwrap();
        let old = &self.values;

        Writer {
            old,
            new,
            span: &self.span,
            db_acquire: &self.db_acquire,
            db_read_begin: &self.db_read_begin,
            db_write_begin: &self.db_write_begin,
        }
    }
}

pub(crate) struct Writer<'a> {
    old: &'a Mutex<Values>,
    new: Values,
    span: &'a Span,
    db_acquire: &'a Span,
    db_read_begin: &'a Span,
    db_write_begin: &'a Span,
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
            self.new
                .db_acquire_durations
                .write_to_span(&self.db_acquire);
        }

        if self.new.db_read_begin_durations != old.db_read_begin_durations {
            self.new
                .db_read_begin_durations
                .write_to_span(&self.db_read_begin);
        }

        if self.new.db_write_begin_durations != old.db_write_begin_durations {
            self.new
                .db_write_begin_durations
                .write_to_span(&self.db_write_begin);
        }

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

    fn write_to_span(&self, span: &Span) {
        state_monitor!(
            parent: span,
            n0_below_200ms = self.below_200ms,
            n1_from_200ms_to_500ms = self.from_200ms_to_500ms,
            n2_from_500ms_to_1s = self.from_500ms_to_1s,
            n3_from_1s_to_3s = self.from_1s_to_3s,
            n4_from_3s_to_10s = self.from_3s_to_10s,
            n5_from_10s_to_30s = self.from_10s_to_30s,
            n6_more_than_30s = self.more_than_30s
        );
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
