use ouisync::{Progress, Repository};
use std::{
    sync::{Arc, Mutex as BlockingMutex},
    time::{Duration, Instant},
};
use tokio::{
    select,
    sync::{broadcast::error::RecvError, mpsc},
    task, time,
};

/// Reports total sync progress of a group of actors.
#[derive(Clone)]
pub struct ProgressReporter {
    tx: mpsc::Sender<Command>,
}

impl ProgressReporter {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(1024);
        task::spawn(handle(rx));
        Self { tx }
    }

    pub async fn run(self, repo: &Repository) {
        self.tx.send(Command::Join).await.unwrap();

        let mut rx = repo.subscribe();
        let mut old = Progress::default();

        loop {
            let new = repo.sync_progress().await.unwrap();
            self.tx.send(Command::Record { new, old }).await.unwrap();
            old = new;

            match rx.recv().await {
                Ok(_) | Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    }
}

async fn handle(mut rx: mpsc::Receiver<Command>) {
    let mut progress = Progress::default();
    let mut num_actors = 0;
    let mut num_synced = 0;
    let mut change = false;
    let mut wakeup = Instant::now();

    loop {
        let command = select! {
            Some(command) = rx.recv() => command,
            _ = time::sleep_until(wakeup.into()) => Command::Report,
            else => break,
        };

        match command {
            Command::Join => {
                num_actors += 1;
            }
            Command::Record { old, new } => {
                progress = sub(progress, old);
                progress = add(progress, new);

                if is_complete(&old) {
                    num_synced -= 1;
                }

                if is_complete(&new) {
                    num_synced += 1;
                }

                change = true;
            }
            Command::Report => {
                if change {
                    report(progress, num_synced, num_actors)
                }

                change = false;
                wakeup = Instant::now() + Duration::from_secs(1);
            }
        }
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

fn report(progress: Progress, num_synced: usize, num_actors: usize) {
    if event_enabled!(tracing::Level::INFO) {
        info!(
            "progress: {:.2} ({}/{})",
            progress.percent(),
            num_synced,
            num_actors
        )
    } else {
        println!(
            "progress: {:.2} ({}/{})",
            progress.percent(),
            num_synced,
            num_actors
        )
    }
}

enum Command {
    Join,
    Record { old: Progress, new: Progress },
    Report,
}
