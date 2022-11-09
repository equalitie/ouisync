mod utils;

use camino::Utf8Path;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use rand::{rngs::StdRng, SeedableRng};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::runtime::Runtime;

criterion_group!(default, write_file, read_file);
criterion_main!(default);

fn write_file(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let file_size = 1024 * 1024;
    let buffer_size = 4096;
    let file_name = Utf8Path::new("file.dat");

    let mut group = c.benchmark_group("write_file");
    group.sample_size(50);
    group.throughput(Throughput::Bytes(file_size));
    group.bench_function(
        BenchmarkId::from_parameter(format!("{}@{}", file_size, buffer_size)),
        |b| {
            b.to_async(&runtime).iter_custom(|iters| async move {
                let mut elapsed = Duration::ZERO;

                for _ in 0..iters {
                    // Setup
                    let mut rng = StdRng::from_entropy();
                    let base_dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
                    let repo = utils::create_repo(&mut rng, &base_dir.path().join("repo.db")).await;

                    // Body
                    let time = Instant::now();
                    utils::write_file(&mut rng, &repo, file_name, file_size as usize, buffer_size)
                        .await;
                    elapsed += time.elapsed();

                    // Teardown
                    repo.close().await;
                }

                elapsed
            })
        },
    );
    group.finish();
}

fn read_file(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let file_size = 1024 * 1024;
    let buffer_size = 4096;
    let file_name = Utf8Path::new("file.dat");

    let mut group = c.benchmark_group("read_file");
    group.sample_size(50);
    group.throughput(Throughput::Bytes(file_size));
    group.bench_function(
        BenchmarkId::from_parameter(format!("{}@{}", file_size, buffer_size)),
        |b| {
            b.to_async(&runtime).iter_custom(|iters| async move {
                let mut elapsed = Duration::ZERO;

                for _ in 0..iters {
                    // Setup
                    let mut rng = StdRng::from_entropy();
                    let base_dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
                    let repo = utils::create_repo(&mut rng, &base_dir.path().join("repo.db")).await;
                    utils::write_file(&mut rng, &repo, file_name, file_size as usize, buffer_size)
                        .await;

                    // Body
                    let time = Instant::now();
                    let _len = utils::read_file(&repo, file_name, buffer_size).await;
                    elapsed += time.elapsed();

                    // Teardown
                    repo.close().await;
                }

                elapsed
            })
        },
    );
    group.finish();
}
