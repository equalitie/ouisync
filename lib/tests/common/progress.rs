use super::actor;
use futures_util::StreamExt;
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressState, ProgressStyle};
use ouisync::{network::Network, Progress, Repository, BLOCK_SIZE};
use std::{
    fmt::Write,
    io::{self, Stderr, Stdout},
    sync::{Arc, Mutex as BlockingMutex},
    time::{Duration, Instant},
};
use tokio::{select, sync::broadcast::error::RecvError, time};

const REPORT_INTERVAL: Duration = Duration::from_secs(1);

/// Reports total sync progress of a group of actors.
#[derive(Clone)]
pub struct ProgressReporter {
    all_progress: Arc<BlockingMutex<Progress>>,
    bars: MultiProgress,
    all_bar: ProgressBar,
}

impl ProgressReporter {
    pub fn new() -> Self {
        let all_progress = Arc::new(BlockingMutex::new(Progress::default()));
        let bars = MultiProgress::new();

        let all_bar = bars.add(ProgressBar::new(1).with_style(all_progress_style()));
        all_bar.set_prefix("total");

        Self {
            all_progress,
            bars,
            all_bar,
        }
    }

    pub fn stdout_writer(&self) -> MakeWriter<fn() -> Stdout> {
        MakeWriter {
            bars: self.bars.clone(),
            inner: io::stdout,
        }
    }

    pub fn stderr_writer(&self) -> MakeWriter<fn() -> Stderr> {
        MakeWriter {
            bars: self.bars.clone(),
            inner: io::stderr,
        }
    }

    pub async fn run(mut self, repo: &Repository) {
        let mut rx = repo.subscribe();

        let one_bar = self.bars.add(
            ProgressBar::new(1)
                .with_style(one_progress_style())
                .with_prefix(actor::name()),
        );
        let _finisher = ProgressBarFinisher(&one_bar);

        let mut old_one_progress = Progress::default();

        loop {
            let new_one_progress = repo.sync_progress().await.unwrap();

            let all_progress = {
                let mut all_progress = self.all_progress.lock().unwrap();
                *all_progress = sub(*all_progress, old_one_progress);
                *all_progress = add(*all_progress, new_one_progress);
                *all_progress
            };

            old_one_progress = new_one_progress;

            one_bar.set_length((old_one_progress.total * BLOCK_SIZE as u64).max(1));
            one_bar.set_position(old_one_progress.value * BLOCK_SIZE as u64);

            self.all_bar
                .set_length((all_progress.total * BLOCK_SIZE as u64).max(1));
            self.all_bar
                .set_position((all_progress.value * BLOCK_SIZE as u64).max(1));

            match rx.recv().await {
                Ok(_) | Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    }
}

impl Drop for ProgressReporter {
    fn drop(&mut self) {
        if Arc::strong_count(&self.all_progress) <= 1 {
            let _ = ProgressBarFinisher(&self.all_bar);
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

fn all_progress_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{prefix:5} [{elapsed_precise}] [{wide_bar:.green.bold/blue}] {percent_precise}%",
    )
    .unwrap()
    .progress_chars("#>-")
}

fn one_progress_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{prefix:5} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes}",
    )
    .unwrap()
    .progress_chars("#>-")
}

struct ProgressBarFinisher<'a>(&'a ProgressBar);

impl Drop for ProgressBarFinisher<'_> {
    fn drop(&mut self) {
        self.0.finish();
    }
}

pub struct Writer<W> {
    bars: MultiProgress,
    inner: W,
}

impl<W> io::Write for Writer<W>
where
    W: io::Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bars.suspend(|| self.inner.write(buf))
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        self.bars.suspend(|| self.inner.write_vectored(bufs))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.bars.suspend(|| self.inner.flush())
    }
}

pub struct MakeWriter<T> {
    bars: MultiProgress,
    inner: T,
}

impl<'w, T> tracing_subscriber::fmt::MakeWriter<'w> for MakeWriter<T>
where
    T: tracing_subscriber::fmt::MakeWriter<'w>,
{
    type Writer = Writer<T::Writer>;

    fn make_writer(&'w self) -> Self::Writer {
        Writer {
            bars: self.bars.clone(),
            inner: self.inner.make_writer(),
        }
    }
}
