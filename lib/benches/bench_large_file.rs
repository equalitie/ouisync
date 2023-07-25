use ouisync::StateMonitor;
use rand::{rngs::StdRng, SeedableRng};
use std::time::Instant;
use tempfile::TempDir;
use tokio::runtime::Runtime;

mod utils;

const FILE_SIZE: usize = 1024 * 1024 * 1024; // 1GiB

const BUFFER_SIZE: usize = 1024 * 1024;

fn main() {
    let runtime = Runtime::new().unwrap();

    let mut rng = StdRng::from_entropy();
    let base_dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let state_monitor = StateMonitor::make_root();

    let repo = runtime.block_on(utils::create_repo(
        &mut rng,
        &base_dir.path().join("repo.db"),
        0,
        state_monitor,
    ));

    println!("writing...");
    let start = Instant::now();
    runtime.block_on(utils::write_file(
        &mut rng,
        &repo,
        "large.dat".into(),
        FILE_SIZE,
        BUFFER_SIZE,
        true,
    ));
    let write_elapsed = start.elapsed();

    println!("reading...");
    let start = Instant::now();
    runtime.block_on(utils::read_file(&repo, "large.dat".into(), BUFFER_SIZE));
    let read_elapsed = start.elapsed();

    let write_elapsed_s = write_elapsed.as_secs_f64();
    let read_elapsed_s = read_elapsed.as_secs_f64();
    println!(
        "write | time: {:.2} s, throughput: {:.2} MiB/s",
        write_elapsed_s,
        (FILE_SIZE / 1024 / 1024) as f64 / write_elapsed_s
    );
    println!(
        "read  | time: {:.2} s, throughput: {:.2} MiB/s",
        read_elapsed_s,
        (FILE_SIZE / 1024 / 1024) as f64 / read_elapsed_s
    );
}
