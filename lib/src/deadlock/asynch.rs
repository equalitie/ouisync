use crate::deadlock::{DeadlockGuard, DeadlockTracker};
use scoped_task::{spawn, ScopedJoinHandle};
use std::{
    fmt,
    future::Future,
    panic::Location,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::time::sleep;

//------------------------------------------------------------------------------

/// Replacement for `tokio::sync::Mutex` instrumented for deadlock detection.
pub struct Mutex<T> {
    inner: tokio::sync::Mutex<T>,
    deadlock_tracker: DeadlockTracker,
}

impl<T> Mutex<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: tokio::sync::Mutex::new(value),
            deadlock_tracker: DeadlockTracker::new(),
        }
    }

    // NOTE: using `track_caller` so that the `Location` constructed inside points to where
    // this function is called and not inside it. Also using `impl Future` return instead of
    // `async fn` because `track_caller` doesn't work correctly with `async`.
    #[track_caller]
    pub fn lock(&self) -> impl Future<Output = MutexGuard<'_, T>> {
        DeadlockGuard::wrap(self.inner.lock(), self.deadlock_tracker.clone())
    }
}

pub type MutexGuard<'a, T> = DeadlockGuard<tokio::sync::MutexGuard<'a, T>>;

//------------------------------------------------------------------------------

static NEXT_EXPECT_SHORT_LIFETIME_ID: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct ExpectShortLifetime {
    id: usize,
    start_time: Instant,
    shared: Arc<std::sync::Mutex<bool>>,
    _watcher: ScopedJoinHandle<()>,
}

impl ExpectShortLifetime {
    pub fn new(max_lifetime: Duration, file_and_line: &'static Location<'static>) -> Self {
        let id = NEXT_EXPECT_SHORT_LIFETIME_ID.fetch_add(1, Ordering::SeqCst);

        let start_time = Instant::now();
        let shared = Arc::new(std::sync::Mutex::new(false));

        Self {
            id,
            start_time,
            shared: shared.clone(),
            _watcher: spawn(async move {
                sleep(max_lifetime).await;

                let mut lock = shared.lock().unwrap();

                if !*lock {
                    *lock = true;
                    println!("ExpectShortLifetime: !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
                    println!(
                        "ExpectShortLifetime: Expected short lifetime, but exceeded {:?} (ID:{})",
                        max_lifetime, id
                    );
                    println!("{:?}", file_and_line);
                    println!("ExpectShortLifetime: !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
                }
            }),
        }
    }
}

impl Drop for ExpectShortLifetime {
    fn drop(&mut self) {
        let mut lock = self.shared.lock().unwrap();

        if !*lock {
            // The watcher did not print yet, mark it so it won't in the future.
            *lock = true;
        } else {
            // The watcher printed the message.
            println!(
                "ExpectShortLifetime: Previously reported task with ID:{} eventually took {:?} to finish",
                self.id,
                self.start_time.elapsed()
            );
        }
    }
}

impl fmt::Debug for ExpectShortLifetime {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ExpectShortLifetime")
    }
}
