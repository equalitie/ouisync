use scoped_task::{spawn, ScopedJoinHandle};
use std::{
    panic::Location,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::time::sleep;

static NEXT_EXPECT_SHORT_LIFETIME_ID: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct ExpectShortLifetime {
    id: usize,
    start_time: Instant,
    shared: Arc<std::sync::Mutex<bool>>,
    _watcher: ScopedJoinHandle<()>,
}

impl ExpectShortLifetime {
    pub fn new(max_lifetime: Duration, location: &'static Location<'static>) -> Self {
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
                    tracing::warn!(
                        %location,
                        id,
                        "ExpectShortLifetime: Expected short lifetime, but exceeded {:?}",
                        max_lifetime,
                    );
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
            tracing::warn!(
                id = self.id,
                "ExpectShortLifetime: Previously reported task eventually took {:?} to finish",
                self.start_time.elapsed()
            );
        }
    }
}
