use super::Context;
use scoped_task::{spawn, ScopedJoinHandle};
use std::{
    panic::Location,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::time::sleep;

static NEXT_EXPECT_SHORT_LIFETIME_ID: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct ExpectShortLifetime {
    id: usize,
    start_time: Instant,
    fired: Arc<AtomicBool>,
    _watcher: ScopedJoinHandle<()>,
}

impl ExpectShortLifetime {
    pub fn new(max_lifetime: Duration, location: &'static Location<'static>) -> Self {
        let id = NEXT_EXPECT_SHORT_LIFETIME_ID.fetch_add(1, Ordering::SeqCst);

        let start_time = Instant::now();
        let fired = Arc::new(AtomicBool::new(false));
        let context = Context::new(location);

        Self {
            id,
            start_time,
            fired: fired.clone(),
            _watcher: spawn(async move {
                sleep(max_lifetime).await;

                if !fired.swap(true, Ordering::SeqCst) {
                    tracing::warn!(
                        id,
                        "ExpectShortLifetime: Expected short lifetime, but exceeded {:?} at\n{}",
                        max_lifetime,
                        context,
                    );
                }
            }),
        }
    }
}

impl Drop for ExpectShortLifetime {
    fn drop(&mut self) {
        if self.fired.swap(true, Ordering::SeqCst) {
            // The watcher printed the message.
            tracing::warn!(
                id = self.id,
                "ExpectShortLifetime: Previously reported task eventually took {:?} to finish",
                self.start_time.elapsed()
            );
        }
    }
}
