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

        map(self.inner.lock(), |inner| MutexGuard { id, inner })
    }
}

pub struct MutexGuard<'a, T: ?Sized + 'a> {
    id: Id,
    inner: sync::MutexGuard<'a, T>,
}

impl<T> Drop for MutexGuard<'_, T>
where
    T: ?Sized,
{
    fn drop(&mut self) {
        super::cancel(self.id);
    }
}

impl<T> Deref for MutexGuard<'_, T>
where
    T: ?Sized,
{
    type Target = T;

    fn deref(&self) -> &T {
        self.inner.deref()
    }
}

impl<T> DerefMut for MutexGuard<'_, T>
where
    T: ?Sized,
{
    fn deref_mut(&mut self) -> &mut T {
        self.inner.deref_mut()
    }
}

/// A RwLock that reports to the standard output when it's not released within WARNING_TIMEOUT
/// duration.
pub struct RwLock<T: ?Sized> {
    inner: sync::RwLock<T>,
}

impl<T> RwLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            inner: sync::RwLock::new(value),
        }
    }

    pub fn read(&self) -> sync::LockResult<RwLockReadGuard<'_, T>> {
        let context = Context::new(Location::caller());
        let id = super::schedule(WARNING_TIMEOUT, context);

        map(self.inner.read(), move |inner| RwLockReadGuard {
            id,
            inner,
        })
    }

    pub fn write(&self) -> sync::LockResult<RwLockWriteGuard<'_, T>> {
        let context = Context::new(Location::caller());
        let id = super::schedule(WARNING_TIMEOUT, context);

        map(self.inner.write(), move |inner| RwLockWriteGuard {
            id,
            inner,
        })
    }
}

pub struct RwLockReadGuard<'a, T>
where
    T: ?Sized + 'a,
{
    id: Id,
    inner: sync::RwLockReadGuard<'a, T>,
}

impl<T> Drop for RwLockReadGuard<'_, T>
where
    T: ?Sized,
{
    fn drop(&mut self) {
        super::cancel(self.id);
    }
}

impl<T> Deref for RwLockReadGuard<'_, T>
where
    T: ?Sized,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

pub struct RwLockWriteGuard<'a, T>
where
    T: ?Sized + 'a,
{
    id: Id,
    inner: sync::RwLockWriteGuard<'a, T>,
}

impl<T> Drop for RwLockWriteGuard<'_, T>
where
    T: ?Sized,
{
    fn drop(&mut self) {
        super::cancel(self.id);
    }
}

impl<T> Deref for RwLockWriteGuard<'_, T>
where
    T: ?Sized,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

impl<T> DerefMut for RwLockWriteGuard<'_, T>
where
    T: ?Sized,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.deref_mut()
    }
}

fn map<S, T, F>(result: sync::LockResult<S>, f: F) -> sync::LockResult<T>
where
    F: FnOnce(S) -> T,
{
    match result {
        Ok(inner) => Ok(f(inner)),
        Err(error) => Err(sync::PoisonError::new(f(error.into_inner()))),
    }
}
