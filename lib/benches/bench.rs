mod utils;

use camino::Utf8Path;
use criterion::{
    criterion_group, criterion_main, BenchmarkId, Criterion, SamplingMode, Throughput,
};
use rand::{rngs::StdRng, SeedableRng};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::runtime::Runtime;

criterion_group!(default, write_file);
criterion_main!(default);

fn write_file(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let file_size = 1024 * 1024;
    let buffer_size = 4096;

    let mut group = c.benchmark_group("write_file");
    group.throughput(Throughput::Bytes(file_size));
    group.bench_function(BenchmarkId::from_parameter(file_size), |b| {
        b.to_async(&runtime).iter_custom(|iters| async move {
            let mut elapsed = Duration::ZERO;

            for _ in 0..iters {
                // Setup is not measured
                let mut rng = StdRng::seed_from_u64(0);
                let base_dir = TempDir::new().unwrap();
                let repo = utils::create_repo(&mut rng, &base_dir.path().join("repo.db")).await;

                // Only this part is measured
                let time = Instant::now();
                utils::write_file(
                    &mut rng,
                    &repo,
                    Utf8Path::new("file.dat"),
                    file_size as usize,
                    buffer_size,
                )
                .await;
                elapsed += time.elapsed();

                // Teardown isn't measured either
                repo.close().await;
            }

            elapsed
        })
    });
    group.finish();
}
