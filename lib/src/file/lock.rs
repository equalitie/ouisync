//! Innternal state of Blob

use crate::event::{Event, Payload};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    sync::Arc,
};
use tokio::sync::broadcast;

/// Lock that indicates that a file is currently being open. When this lock exists, certain
/// operations on the file (e.g. forking over it) are prohibited. This lock is already created in
/// acquired state and is released by dropping it.
pub(crate) struct OpenLock {
    event_tx: broadcast::Sender<Event>,
    write_lock: AtomicBool,
}

impl OpenLock {
    pub(super) fn new(event_tx: broadcast::Sender<Event>) -> Arc<Self> {
        Arc::new(Self {
            event_tx,
            write_lock: AtomicBool::new(false),
        })
    }
}

impl Drop for OpenLock {
    fn drop(&mut self) {
        self.event_tx
            .send(Event::new(Payload::FileClosed))
            .unwrap_or(0);
    }
}

/// Lock that protects a file from being written to concurrently. This lock is created in released
/// state and must be explicitly acquired by calling `acquired`. Once acquired, it's only released
/// when the contained `OpenLock` is also released, that is, when all instances of the file are
/// dropped.
pub(super) struct WriteLock {
    open_lock: Arc<OpenLock>,
    acquired: bool,
}

impl WriteLock {
    pub fn new(open_lock: Arc<OpenLock>) -> Self {
        Self {
            open_lock,
            acquired: false,
        }
    }

    pub fn acquire(&mut self) -> bool {
        if self.acquired {
            return true;
        }

        if !self.open_lock.write_lock.fetch_or(true, Ordering::Relaxed) {
            self.acquired = true;
            return true;
        }

        false
    }
}
