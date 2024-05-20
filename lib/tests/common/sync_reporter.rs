use ouisync::{Progress, Repository};
use std::{
    sync::{Arc, Mutex as BlockingMutex},
    time::{Duration, Instant},
};
use tokio::sync::broadcast::error::RecvError;

const REPORT_INTERVAL: Duration = Duration::from_secs(1);

/// Reports total sync progress of a group of actors.
#[derive(Clone)]
pub struct SyncReporter {
    shared: Arc<BlockingMutex<Shared>>,
    progress: Progress,
    prefix: String,
}

struct Shared {
    progress: Progress,
    report_timestamp: Instant,
}

impl SyncReporter {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(BlockingMutex::new(Shared {
                progress: Progress::default(),
                report_timestamp: Instant::now(),
            })),
            progress: Progress::default(),
            prefix: String::new(),
        }
    }

    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    pub async fn run(mut self, repo: &Repository) {
        let mut rx = repo.subscribe();

        loop {
            let progress = repo.sync_progress().await.unwrap();
            self.report(progress);

            match rx.recv().await {
                Ok(_) | Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    }

    fn report(&mut self, progress: Progress) {
        let mut shared = self.shared.lock().unwrap();

        shared.progress = sub(shared.progress, self.progress);
        shared.progress = add(shared.progress, progress);
        self.progress = progress;

        let now = Instant::now();
        if (now.duration_since(shared.report_timestamp) < REPORT_INTERVAL) {
            return;
        }

        println!("{}{}", self.prefix, shared.progress.percent());

        shared.report_timestamp = now;
    }
}

fn add(a: Progress, b: Progress) -> Progress {
    Progress {
        value: a.value + b.value,
        total: a.total + b.total,
    }
}

fn sub(a: Progress, b: Progress) -> Progress {
    Progress {
        value: a.value - b.value,
        total: a.total - b.total,
    }
}

fn is_complete(progress: &Progress) -> bool {
    progress.total > 0 && progress.value >= progress.total
}
