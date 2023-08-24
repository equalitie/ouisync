// Probably false positive triggered by `task_local`
#![allow(clippy::declare_interior_mutable_const)]

use crate::{crypto::sign::PublicKey, protocol::BlockId};
use core::fmt;
use futures_util::{stream, Stream};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::broadcast;

#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Payload {
    /// A new snapshot was created in the specified branch.
    BranchChanged(PublicKey),
    /// A block with the specified id referenced from the specified branch was received from a
    /// remote replica.
    BlockReceived {
        block_id: BlockId,
        branch_id: PublicKey,
    },
    /// The `maintain` worker job successfully completed. It won't perform any more work until
    /// triggered again by any of the above events.
    /// This event is useful mostly for diagnostics or testing and can be safely ignored in other
    /// contexts.
    MaintenanceCompleted,
}

/// Notification event
#[derive(Copy, Clone, Debug)]
pub struct Event {
    /// Event payload.
    pub payload: Payload,
    /// Event scope. Can be used to distinguish which part of the code the event was emitted from.
    /// Scope can be set by running the event-emitting task with `EventScope::apply`. If no scope
    /// is set, uses `EventScope::DEFAULT`.
    pub(crate) scope: EventScope,
}

impl Event {
    pub(crate) fn new(payload: Payload) -> Self {
        Self {
            payload,
            scope: EventScope::DEFAULT,
        }
    }

    pub(crate) fn with_scope(self, scope: EventScope) -> Self {
        Self { scope, ..self }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub(crate) struct EventScope(usize);

impl EventScope {
    pub const DEFAULT: Self = Self(0);

    /// Creates new scope.
    pub fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone)]
pub(crate) struct EventSender {
    inner: broadcast::Sender<Event>,
    scope: EventScope,
}

impl EventSender {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: broadcast::channel(capacity).0,
            scope: EventScope::DEFAULT,
        }
    }

    pub fn with_scope(self, scope: EventScope) -> Self {
        Self { scope, ..self }
    }

    pub fn send(&self, payload: Payload) {
        self.inner
            .send(Event::new(payload).with_scope(self.scope))
            .unwrap_or(0);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.subscribe()
    }
}

#[derive(Debug)]
pub(crate) struct Lagged;

impl fmt::Display for Lagged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "event channel lagged")
    }
}

impl std::error::Error for Lagged {}

/// Converts event receiver into a `Stream`.
pub(crate) fn into_stream(
    rx: broadcast::Receiver<Event>,
) -> impl Stream<Item = Result<Event, Lagged>> {
    stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Ok(event) => Some((Ok(event), rx)),
            Err(broadcast::error::RecvError::Lagged(_)) => Some((Err(Lagged), rx)),
            Err(broadcast::error::RecvError::Closed) => None,
        }
    })
}
