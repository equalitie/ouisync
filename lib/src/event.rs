// Probably false positive triggered by `task_local`
#![allow(clippy::declare_interior_mutable_const)]

use crate::crypto::sign::PublicKey;
use std::{
    future::Future,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::{sync::broadcast, task_local};

task_local! {
    static CURRENT_SCOPE: EventScope;
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum Payload {
    /// A new snapshot was created or a block received in the specified branch.
    BranchChanged(PublicKey),
    /// A file was closed
    FileClosed,
}

/// Notification event
#[derive(Copy, Clone, Debug)]
pub struct Event {
    /// Event payload.
    pub(crate) payload: Payload,
    /// Event scope. Can be used to distinguish which part of the code the event was emitted from.
    /// Scope can be set by running the event-emitting task with `EventScope::apply`. If no scope
    /// is set, uses `EventScope::DEFAULT`.
    pub(crate) scope: EventScope,
}

impl Event {
    pub(crate) fn new(payload: Payload) -> Self {
        let scope = CURRENT_SCOPE
            .try_with(|scope| *scope)
            .unwrap_or(EventScope::DEFAULT);

        Self { payload, scope }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub(crate) struct EventScope(usize);

impl EventScope {
    const DEFAULT: Self = Self(0);

    /// Creates new scope.
    pub fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    pub async fn apply<F: Future>(self, f: F) -> F::Output {
        CURRENT_SCOPE.scope(self, f).await
    }
}

/// Receiver adapter that receives only `BranchChanged` events.
pub struct BranchChangedReceiver {
    inner: broadcast::Receiver<Event>,
}

impl BranchChangedReceiver {
    pub(crate) fn new(inner: broadcast::Receiver<Event>) -> Self {
        Self { inner }
    }

    pub async fn recv(&mut self) -> Result<PublicKey, broadcast::error::RecvError> {
        loop {
            match self.inner.recv().await {
                Ok(Event {
                    payload: Payload::BranchChanged(branch_id),
                    ..
                }) => break Ok(branch_id),
                Ok(Event {
                    payload: Payload::FileClosed,
                    ..
                }) => continue,
                Err(error) => break Err(error),
            }
        }
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
