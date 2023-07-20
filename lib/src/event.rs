// Probably false positive triggered by `task_local`
#![allow(clippy::declare_interior_mutable_const)]

use crate::{crypto::sign::PublicKey, protocol::BlockId};
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

/// Receiver adapter that skips events from the given scope.
pub(crate) struct IgnoreScopeReceiver {
    inner: broadcast::Receiver<Event>,
    scope: EventScope,
}

impl IgnoreScopeReceiver {
    pub fn new(inner: broadcast::Receiver<Event>, scope: EventScope) -> Self {
        Self { inner, scope }
    }

    pub async fn recv(&mut self) -> Result<Payload, broadcast::error::RecvError> {
        loop {
            match self.inner.recv().await {
                Ok(Event { payload, scope }) if scope != self.scope => break Ok(payload),
                Ok(_) => continue,
                Err(error) => break Err(error),
            }
        }
    }
}
