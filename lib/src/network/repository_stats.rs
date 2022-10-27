use std::ops::{Deref, DerefMut};
use tracing::Span;

pub(super) struct RepositoryStats {
    values: Values,
    span: Span,
}

impl RepositoryStats {
    pub fn new(span: Span) -> Self {
        Self {
            values: Values::default(),
            span,
        }
    }

    pub fn write(&mut self) -> Writer {
        let new = self.values;
        let old = &mut self.values;

        Writer {
            old,
            new,
            span: &self.span,
        }
    }
}

pub(super) struct Writer<'a> {
    old: &'a mut Values,
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
        if self.new.index_requests_inflight != self.old.index_requests_inflight {
            tracing::trace!(parent: self.span, index_requests_inflight = self.new.index_requests_inflight);
        }

        if self.new.block_requests_inflight != self.old.block_requests_inflight {
            tracing::trace!(parent: self.span, block_requests_inflight = self.new.block_requests_inflight);
        }

        if self.new.total_requests_cummulative != self.old.total_requests_cummulative {
            tracing::trace!(parent: self.span, total_requests_cummulative = self.new.total_requests_cummulative);
        }

        *self.old = self.new;
    }
}

#[derive(Copy, Clone, Default)]
pub(super) struct Values {
    // This indicates how many requests for index nodes are currently in flight.  It is used by the
    // UI to indicate that the index is being synchronized.
    pub index_requests_inflight: u64,
    pub block_requests_inflight: u64,
    pub total_requests_cummulative: u64,
}
