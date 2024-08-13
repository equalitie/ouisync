//! Simulation of a swarm of Ouisync instances sharing a repository. Useful for benchmarking sync
//! performance.

#[path = "../tests/common/mod.rs"]
#[macro_use]
mod common;

#[path = "utils/summary.rs"]
mod summary;

use clap::Parser;
use common::{actor, progress::ProgressReporter, sync_watch, Env, Proto, DEFAULT_REPO};
use metrics::{Counter, Gauge, Recorder};
use ouisync::{Access, AccessMode, File, Network, Repository, TrafficStats};
use rand::{distributions::Standard, rngs::StdRng, Rng, SeedableRng};
use std::{
    fmt,
    fs::OpenOptions,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
    time::Duration,
};
use summary::SummaryRecorder;
use tokio::{select, sync::Barrier, time};

mod future {
    pub use futures_util::future::{join, join_all};
    pub use std::future::pending;
}

fn main() -> ExitCode {
    let options = Options::parse();
    if options.num_writers == 0 {
        eprintln!("error: at least one write replica required");
        return ExitCode::FAILURE;
    }

    let actors: Vec<_> = (0..options.num_writers)
        .map(|index| ActorId {
            access_mode: AccessMode::Write,
            index,
        })
        .chain((0..options.num_readers).map(|index| ActorId {
            access_mode: AccessMode::Read,
            index,
        }))
        .chain((0..options.num_blinds).map(|index| ActorId {
            access_mode: AccessMode::Blind,
            index,
        }))
        .collect();
    let proto = options.protocol;

    let mut env = Env::new();

    // Wait until everyone is fully synced.
    let watch_txs: Vec<_> = (0..options.num_writers)
        .map(|_| sync_watch::Sender::new())
        .collect();
    let watch_rxs: Vec<_> = watch_txs.iter().map(|tx| tx.subscribe()).collect();
    let mut watch_txs = watch_txs.into_iter();

    // Then wait until everyone is done. This is so that even actors that finished syncing still
    // remain online for other actors to sync from.
    let barrier = Arc::new(Barrier::new(actors.len()));

    let summary_recorder = SummaryRecorder::new();
    let progress_reporter = {
        let _enter = env.runtime().enter();
        options.progress.then(ProgressReporter::new)
    };

    let files: Vec<_> = options
        .file_sizes
        .iter()
        .copied()
        .enumerate()
        .map(|(index, size)| FileParams {
            index,
            size,
            seed: 0,
        })
        .collect();

    for actor in actors.iter().copied() {
        let other_actors: Vec<_> = actors
            .iter()
            .filter(|other_actor| **other_actor != actor)
            .copied()
            .collect();

        // If we are writer, grab the watch sender.
        let watch_tx = (actor.access_mode == AccessMode::Write)
            .then(|| watch_txs.next())
            .flatten();
        // If we are writer, grab all watch receivers except the one corresponding to us (no need
        // to watch ourselves). Otherwise grab them all.
        let watch_rxs: Vec<_> = watch_rxs
            .iter()
            .enumerate()
            .filter(|(index, _)| actor.access_mode != AccessMode::Write || *index != actor.index)
            .map(|(_, rx)| rx.clone())
            .collect();

        let files = files.clone();

        let barrier = barrier.clone();
        let progress_reporter = progress_reporter.clone();
        let summary_recorder = summary_recorder.actor();

        env.actor(&actor.to_string(), async move {
            let network = actor::create_network(proto).await;

            let recorder = actor::get_default_recorder();
            let progress_monitor = ProgressMonitor::new(&recorder);
            let throughput_monitor = ThroughputMonitor::new(&recorder);

            // Create the repo
            let params = actor::get_repo_params(DEFAULT_REPO).with_recorder(recorder);
            let secrets = actor::get_repo_secrets(DEFAULT_REPO);
            let repo = Repository::create(
                &params,
                Access::new(None, None, secrets.with_mode(actor.access_mode)),
            )
            .await
            .unwrap();
            let _reg = network.register(repo.handle()).await;

            // Create the file
            if actor.access_mode == AccessMode::Write {
                if let Some(file_params) = files.get(actor.index) {
                    let mut file = repo.create_file(file_params.name()).await.unwrap();
                    write_random_file(&mut file, file_params.seed, file_params.size).await;
                }
            }

            // Connect to the other peers
            for other_actor in other_actors {
                let addr = actor::lookup_addr(&other_actor.to_string()).await;
                network.add_user_provided_peer(&addr);
            }

            let run_sync = async {
                // Wait until fully synced
                let run_watch_rxs = future::join_all(watch_rxs.into_iter().map(|rx| rx.run(&repo)));

                if let Some(watch_tx) = watch_tx {
                    future::join(watch_tx.run(&repo), run_watch_rxs).await;
                } else {
                    run_watch_rxs.await;
                }

                // Wait until everyone finished
                barrier.wait().await;
            };

            let run_progress_reporter = async {
                if let Some(progress_reporter) = progress_reporter {
                    progress_reporter.run(&repo).await
                } else {
                    future::pending().await
                }
            };

            select! {
                _ = run_sync => (),
                _ = run_progress_reporter => (),
                _ = progress_monitor.run(&repo) => (),
                _ = throughput_monitor.run(&network) => (),
            }

            info!("done");

            summary_recorder.record(&network);
        });
    }

    drop(watch_rxs);
    drop(env);
    drop(progress_reporter);

    let summary = summary_recorder.finalize(options.label);

    if let Some(path) = options.output {
        let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("error: failed to open/create {}: {}", path.display(), error);
                return ExitCode::FAILURE;
            }
        };

        serde_json::to_writer(&mut file, &summary).unwrap();
        file.write_all(b"\n").unwrap();
    } else {
        println!();
        serde_json::to_writer_pretty(io::stdout().lock(), &summary).unwrap();
        println!();
    }

    ExitCode::SUCCESS
}

#[derive(Parser, Debug)]
struct Options {
    /// Size of the file(s) to share in bytes. Can use metric (kB, MB, ...) or binary (kiB, MiB, ...)
    /// suffixes. Can take multiple values to create multiple files by multiple writers.
    #[arg(
        short = 's',
        long = "file-size",
        value_delimiter = ',',
        value_name = "SIZE",
        value_parser = parse_size,
        default_values = ["1MiB"]
    )]
    pub file_sizes: Vec<u64>,

    /// Number of replicas with write access. Must be at least 1.
    #[arg(short = 'w', long, default_value_t = 2)]
    pub num_writers: usize,

    /// Number of replicas with read access.
    #[arg(short = 'r', long, default_value_t = 0)]
    pub num_readers: usize,

    /// Number of replicas with blind access.
    #[arg(short = 'b', long, default_value_t = 0)]
    pub num_blinds: usize,

    /// Network protocol to use (QUIC or TCP).
    #[arg(short, long, value_parser, default_value_t = Proto::Quic)]
    pub protocol: Proto,

    /// File to append the summary to. Will be created if not exists. If ommited prints the summary
    /// to stdout.
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Human-readable label for this execution of the benchmark. Useful to distinguish outputs of
    /// mutliple versions of this benchmark.
    #[arg(short, long, default_value_t)]
    pub label: String,

    /// Show progress bar.
    #[arg(long)]
    pub progress: bool,

    // The following arguments may be passed down from `cargo bench` so we need to accept them even
    // if we don't use them.
    #[arg(
        long = "bench",
        hide = true,
        hide_short_help = true,
        hide_long_help = true
    )]
    _bench: bool,

    #[arg(
        long = "profile-time",
        hide = true,
        hide_short_help = true,
        hide_long_help = true
    )]
    _profile_time: Option<String>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct ActorId {
    access_mode: AccessMode,
    index: usize,
}

impl fmt::Display for ActorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}",
            match self.access_mode {
                AccessMode::Write => "w",
                AccessMode::Read => "r",
                AccessMode::Blind => "b",
            },
            self.index
        )
    }
}

#[derive(Copy, Clone)]
struct FileParams {
    index: usize,
    size: u64,
    seed: u64,
}

impl FileParams {
    fn name(&self) -> String {
        format!("file-{}.dat", self.index)
    }
}

fn parse_size(input: &str) -> Result<u64, parse_size::Error> {
    parse_size::parse_size(input)
}

async fn write_random_file(file: &mut File, seed: u64, size: u64) {
    let mut chunk = Vec::new();
    let mut gen = RandomChunks::new(seed, size);

    while gen.next(&mut chunk) {
        file.write_all(&chunk).await.unwrap();
    }

    file.flush().await.unwrap();
}

const CHUNK_SIZE: usize = 4096;

// Generate random byte chunks of `CHUNK_SIZE` up to the given total size.
struct RandomChunks {
    rng: StdRng,
    remaining: usize,
}

impl RandomChunks {
    fn new(seed: u64, size: u64) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
            remaining: size as usize,
        }
    }

    fn next(&mut self, chunk: &mut Vec<u8>) -> bool {
        if self.remaining == 0 {
            return false;
        }

        let chunk_size = CHUNK_SIZE.min(self.remaining);
        self.remaining -= chunk_size;

        chunk.clear();
        chunk.extend(
            (&mut self.rng)
                .sample_iter::<u8, _>(Standard)
                .take(chunk_size),
        );

        true
    }
}

struct ProgressMonitor {
    progress: Gauge,
}

impl ProgressMonitor {
    fn new(recorder: &impl Recorder) -> Self {
        metrics::with_local_recorder(recorder, || Self {
            progress: metrics::gauge!("progress"),
        })
    }

    async fn run(&self, repo: &Repository) {
        use tokio::sync::broadcast::error::RecvError;

        let mut rx = repo.subscribe();

        loop {
            let progress = repo.sync_progress().await.unwrap();
            self.progress.set(progress.ratio());

            match rx.recv().await {
                Ok(_) | Err(RecvError::Lagged(_)) => (),
                Err(RecvError::Closed) => break,
            }
        }
    }
}

struct ThroughputMonitor {
    bytes_sent: Counter,
    bytes_received: Counter,
}

impl ThroughputMonitor {
    fn new(recorder: &impl Recorder) -> Self {
        metrics::with_local_recorder(recorder, || Self {
            bytes_sent: metrics::counter!("bytes_sent"),
            bytes_received: metrics::counter!("bytes_received"),
        })
    }

    async fn run(&self, network: &Network) {
        loop {
            let TrafficStats { send, recv, .. } = network.traffic_stats();

            self.bytes_sent.absolute(send);
            self.bytes_received.absolute(recv);

            time::sleep(Duration::from_millis(250)).await;
        }
    }
}
