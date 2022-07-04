use crate::crypto::sign::PublicKey;
use std::{
    future::Future,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::task_local;

task_local! {
    static CURRENT_SCOPE: EventScope;
}

/// Notification event
#[derive(Copy, Clone, Debug)]
pub struct Event {
    /// Branch that triggered the event.
    pub branch_id: PublicKey,
    /// Event scope. Can be used to distinguish which part of the code the event was emitted from.
    /// Scope can be set by running the event-emitting task with `EventScope::apply`. If no scope
    /// is set, uses `EventScope::DEFAULT`.
    pub(crate) scope: EventScope,
}

impl Event {
    pub(crate) fn new(branch_id: PublicKey) -> Self {
        let scope = CURRENT_SCOPE
            .try_with(|scope| *scope)
            .unwrap_or(Scope::DEFAULT);

        Self { branch_id, scope }
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
