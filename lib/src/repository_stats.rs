use std::{
    ops::{Deref, DerefMut},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tracing::Span;

pub(crate) struct RepositoryStats {
    values: Arc<Mutex<Values>>,
    span: Span,
    request_queue_durations: Span,
    request_inflight_durations: Span,
    db_acquire: Span,
    db_read_begin: Span,
    db_write_begin: Span,
}

impl RepositoryStats {
    pub fn new(span: Span) -> Self {
        let values = Arc::new(Mutex::new(Values::default()));

        let request_queue_durations =
            tracing::info_span!(parent: span.clone(), "request_queue_durations");
        let request_inflight_durations =
            tracing::info_span!(parent: span.clone(), "request_inflight_durations");
        let db_acquire = tracing::info_span!(parent: span.clone(), "db_acquire");
        let db_read_begin = tracing::info_span!(parent: span.clone(), "db_read_begin");
        let db_write_begin = tracing::info_span!(parent: span.clone(), "db_write_begin");

        Self {
            values,
            span,
            request_queue_durations,
            request_inflight_durations,
            db_acquire,
            db_read_begin,
            db_write_begin,
        }
    }

    pub fn write(&self) -> Writer {
        let lock = self.values.lock().unwrap();
        let old = *lock;

        Writer {
            lock,
            old,
            span: &self.span,
            request_queue_durations: &self.request_queue_durations,
            request_inflight_durations: &self.request_inflight_durations,
            db_acquire: &self.db_acquire,
            db_read_begin: &self.db_read_begin,
            db_write_begin: &self.db_write_begin,
        }
    }
}

pub(crate) struct Writer<'a> {
    lock: MutexGuard<'a, Values>,
    old: Values,
    span: &'a Span,
    request_queue_durations: &'a Span,
    request_inflight_durations: &'a Span,
    db_acquire: &'a Span,
    db_read_begin: &'a Span,
    db_write_begin: &'a Span,
}

impl<'a> Deref for Writer<'a> {
    type Target = Values;

    fn deref(&self) -> &Self::Target {
        &*self.lock
    }
}

impl<'a> DerefMut for Writer<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.lock
    }
}

impl<'a> Drop for Writer<'a> {
    fn drop(&mut self) {
        if self.lock.index_requests_inflight != self.old.index_requests_inflight {
            state_monitor!(parent: self.span, index_requests_inflight = self.lock.index_requests_inflight);
        }

        if self.lock.block_requests_inflight != self.old.block_requests_inflight {
            state_monitor!(parent: self.span, block_requests_inflight = self.lock.block_requests_inflight);
        }

        if self.lock.total_requests_cummulative != self.old.total_requests_cummulative {
            state_monitor!(parent: self.span, total_requests_cummulative = self.lock.total_requests_cummulative);
        }

        if self.lock.request_timeouts != self.old.request_timeouts {
            state_monitor!(parent: self.span, request_timeouts = self.lock.request_timeouts);
        }

        if self.lock.db_acquire_durations != self.old.db_acquire_durations {
            self.lock
                .db_acquire_durations
                .write_to_span(&self.db_acquire);
        }

        if self.lock.db_read_begin_durations != self.old.db_read_begin_durations {
            self.lock
                .db_read_begin_durations
                .write_to_span(&self.db_read_begin);
        }

        if self.lock.db_write_begin_durations != self.old.db_write_begin_durations {
            self.lock
                .db_write_begin_durations
                .write_to_span(&self.db_write_begin);
        }

        if self.lock.request_queue_durations != self.old.request_queue_durations {
            self.lock
                .request_queue_durations
                .write_to_span(&self.request_queue_durations);
        }

        if self.lock.request_inflight_durations != self.old.request_inflight_durations {
            self.lock
                .request_inflight_durations
                .write_to_span(&self.request_inflight_durations);
        }
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

    request_queue_durations: DurationRanges,
    request_inflight_durations: DurationRanges,
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

    pub(crate) fn note_request_queue_duration(&mut self, duration: Duration) {
        self.request_queue_durations.note_duration(duration);
    }

    pub(crate) fn note_request_inflight_duration(&mut self, duration: Duration) {
        self.request_inflight_durations.note_duration(duration);
    }
}
