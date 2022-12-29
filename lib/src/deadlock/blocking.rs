use core::ops::{Deref, DerefMut};
use once_cell::sync::Lazy;
use std::{
    backtrace::Backtrace,
    collections::HashMap,
    panic::Location,
    sync::{
        self,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

static WATCHED_ENTRIES: Lazy<sync::Mutex<HashMap<u64, WatchedEntry>>> =
    Lazy::new(|| sync::Mutex::new(HashMap::new()));

static WATCHING_THREAD: Lazy<thread::JoinHandle<()>> = Lazy::new(|| thread::spawn(watching_thread));

static NEXT_ENTRY_ID: AtomicU64 = AtomicU64::new(0);

const WARNING_TIMEOUT: Duration = Duration::from_secs(5);

/// A Mutex that reports to the standard output when it's not released within WARNING_TIMEOUT
/// duration.
pub struct Mutex<T: ?Sized> {
    inner: sync::Mutex<T>,
}

impl<T> Mutex<T> {
    pub fn new(t: T) -> Self {
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
        // Make sure the thread is instantiated. Is it better to do this here or in the
        // `Mutex::new` function?
        let _ = *WATCHING_THREAD;

        let entry_id = NEXT_ENTRY_ID.fetch_add(1, Ordering::SeqCst);

        WATCHED_ENTRIES.lock().unwrap().insert(
            entry_id,
            WatchedEntry {
                id: entry_id,
                deadline: Instant::now() + WARNING_TIMEOUT,
                file_and_line: Location::caller(),
                backtrace: Backtrace::capture(),
            },
        );

        let lock_result = self
            .inner
            .lock()
            .map(|inner| MutexGuard { entry_id, inner })
            .map_err(|err| {
                sync::PoisonError::new(MutexGuard {
                    entry_id,
                    inner: err.into_inner(),
                })
            });

        if lock_result.is_err() {
            // MutexGuard was not created, so we need to remove it ourselves.
            WATCHED_ENTRIES.lock().unwrap().remove(&entry_id);
        }

        lock_result
    }
}

pub struct MutexGuard<'a, T: ?Sized + 'a> {
    entry_id: u64,
    inner: sync::MutexGuard<'a, T>,
}

impl<'a, T: ?Sized> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        if WATCHED_ENTRIES
            .lock()
            .unwrap()
            .remove(&self.entry_id)
            .is_none()
        {
            // Using `println!` and not `tracing::*` to avoid circular dependencies because on
            // Android tracing uses `StateMonitor` which uses these mutexes.
            println!(
                "Previously reported blocking mutex (id:{}) got released.",
                self.entry_id
            );
        }
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

struct WatchedEntry {
    id: u64,
    deadline: Instant,
    file_and_line: &'static Location<'static>,
    backtrace: Backtrace,
}

fn watching_thread() {
    loop {
        thread::sleep(Duration::from_secs(5));

        let now = Instant::now();

        WATCHED_ENTRIES.lock().unwrap().retain(|_, entry| {
            if now > entry.deadline {
                // Using `println!` and not `tracing::*` to avoid circular dependencies because on
                // Android tracing uses `StateMonitor` which uses these mutexes.
                println!(
                    "Possible blocking deadlock (id:{}) at:\n{}\n{}\n",
                    entry.id, entry.file_and_line, entry.backtrace
                );
                false
            } else {
                true
            }
        });
    }
}
