use super::{timer::Id, Context, WARNING_TIMEOUT};
use core::ops::{Deref, DerefMut};
use std::{panic::Location, sync};

/// A Mutex that reports to the standard output when it's not released within WARNING_TIMEOUT
/// duration.
#[derive(Default)]
pub struct Mutex<T: ?Sized> {
    inner: sync::Mutex<T>,
}

impl<T> Mutex<T> {
    pub const fn new(t: T) -> Self {
        Self {
            inner: sync::Mutex::new(t),
        }
    }
}

impl<T: ?Sized> Mutex<T> {
    // NOTE: using `track_caller` so that the `Location` constructed inside points to where
    // this function is called and not inside it.
    #[track_caller]
    pub fn lock(&self) -> sync::LockResult<MutexGuard<'_, T>> {
        let context = Context::new(Location::caller());
        let id = super::schedule(WARNING_TIMEOUT, context);

        let lock_result = self
            .inner
            .lock()
            .map(|inner| MutexGuard { id, inner })
            .map_err(|err| {
                sync::PoisonError::new(MutexGuard {
                    id,
                    inner: err.into_inner(),
                })
            });

        if lock_result.is_err() {
            // MutexGuard was not created, so we need to remove it ourselves.
            super::cancel(id);
        }

        lock_result
    }
}

pub struct MutexGuard<'a, T: ?Sized + 'a> {
    id: Id,
    inner: sync::MutexGuard<'a, T>,
}

impl<'a, T: ?Sized> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        super::cancel(self.id);
    }
}

impl<'a, T: ?Sized> Deref for MutexGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.inner.deref()
    }
}

impl<'a, T: ?Sized> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.inner.deref_mut()
    }
}
