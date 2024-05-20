//! Simulation of a swarm of Ouisync instances sharing a repository. Useful for benchmarking sync
//! performance.

#[path = "../tests/common/mod.rs"]
#[macro_use]
mod common;

use clap::Parser;
use common::{actor, sync_reporter::SyncReporter, sync_watch, Env, Proto, DEFAULT_REPO};
use ouisync::{AccessMode, File};
use rand::{distributions::Standard, rngs::StdRng, Rng, SeedableRng};
use std::{fmt, process::ExitCode, sync::Arc, time::Instant};
use tokio::{select, sync::Barrier};

fn main() -> ExitCode {
    let options = Options::parse();
    if options.num_writers == 0 {
        eprintln!("error: at least one write replica required");
        return ExitCode::FAILURE;
    }

    let file_size = options.file_size;
    let actors: Vec<_> = (0..options.num_writers)
        .map(|i| ActorId(AccessMode::Write, i))
        .chain((0..options.num_readers).map(|i| ActorId(AccessMode::Read, i)))
        .collect();
    let proto = options.protocol;

    let mut env = Env::new();

    // Wait until everyone is fully synced.
    let (watch_tx, watch_rx) = sync_watch::channel();
    let mut watch_tx = Some(watch_tx);

    // Then wait until everyone is done. This is so that even actors that finished syncing still
    // remain online for other actors to sync from.
    let barrier = Arc::new(Barrier::new(actors.len()));

    let reporter = SyncReporter::new();

    let file_name = "file.dat";
    let file_seed = 0;

    for actor in &actors {
        let other_actors: Vec<_> = actors
            .iter()
            .filter(|other_actor| *other_actor != actor)
            .copied()
            .collect();

        let access_mode = actor.0;

        let watch_tx = (actor.0 == AccessMode::Write)
            .then(|| watch_tx.take())
            .flatten();
        let watch_rx = watch_rx.clone();
        let barrier = barrier.clone();
        let reporter = reporter.clone();

        env.actor(&actor.to_string(), async move {
            let network = actor::create_network(proto).await;

            // Connect to the other peers
            for other_actor in other_actors {
                let addr = actor::lookup_addr(&other_actor.to_string()).await;
                network.add_user_provided_peer(&addr);
            }

            // Create the repo
            let repo = actor::create_repo_with_mode(DEFAULT_REPO, access_mode).await;
            let _reg = network.register(repo.handle()).await;

            // One writer creates the file initially.
            if watch_tx.is_some() {
                let mut file = repo.create_file(file_name).await.unwrap();
                write_random_file(&mut file, file_seed, file_size).await;
            }

            let run = async {
                if let Some(watch_tx) = watch_tx {
                    drop(watch_rx);
                    watch_tx.run(&repo).await;
                } else {
                    watch_rx.run(&repo).await;
                }
            };

            select! {
                _ = run => (),
                _ = reporter.run(&repo) => (),
            }

            // Check the file content matches the original file.
            {
                let mut file = repo.open_file(file_name).await.unwrap();
                check_random_file(&mut file, file_seed, file_size).await;
            }

            info!("done");

            barrier.wait().await;
        });
    }

    drop(watch_rx);

    let start = Instant::now();
    info!("simulation started");
    drop(env);
    info!(
        "simulation completed in {:.3}s",
        start.elapsed().as_secs_f64()
    );

    ExitCode::SUCCESS
}

#[derive(Parser, Debug)]
struct Options {
    /// Size of the file to share in bytes. Can use metric (kB, MB, ...) or binary (kiB, MiB, ...)
    /// suffixes.
    #[arg(short = 's', long, value_parser = parse_size, default_value_t = 1024 * 1024)]
    pub file_size: u64,

    /// Number of replicas with write access. Must be at least 1.
    #[arg(short = 'w', long, default_value_t = 2)]
    pub num_writers: usize,

    /// Number of replicas with read access.
    #[arg(short = 'r', long, default_value_t = 0)]
    pub num_readers: usize,

    /// Network protocol to use (QUIC or TCP).
    #[arg(short, long, value_parser, default_value_t = Proto::Quic)]
    pub protocol: Proto,

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
struct ActorId(AccessMode, usize);

impl fmt::Display for ActorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}",
            match self.0 {
                AccessMode::Write => "w",
                AccessMode::Read => "r",
                AccessMode::Blind => "b",
            },
            self.1
        )
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

async fn check_random_file(file: &mut File, seed: u64, size: u64) {
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    let mut gen = RandomChunks::new(seed, size);
    let mut offset = 0;

    while gen.next(&mut expected) {
        actual.clear();
        actual.resize(expected.len(), 0);

        file.read_all(&mut actual).await.unwrap();

        similar_asserts::assert_eq!(
            actual,
            expected,
            "actor: {}, offset: {}",
            actor::name(),
            offset
        );

        offset += expected.len();
    }
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
