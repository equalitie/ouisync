mod utils;

use camino::Utf8Path;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use rand::{rngs::StdRng, SeedableRng};
use tempfile::TempDir;
use tokio::runtime::Runtime;

criterion_group!(default, write_file, read_file);
criterion_main!(default);

fn write_file(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let file_size = 1024 * 1024;
    let buffer_size = 4096;

    let mut group = c.benchmark_group("lib/write_file");
    group.sample_size(50);
    group.throughput(Throughput::Bytes(file_size));
    group.bench_function(
        BenchmarkId::from_parameter(format!("{}@{}", file_size, buffer_size)),
        |b| {
            b.iter_batched_ref(
                || {
                    let mut rng = StdRng::from_entropy();
                    let base_dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
                    let repo = runtime.block_on(utils::create_repo(
                        &mut rng,
                        &base_dir.path().join("repo.db"),
                    ));
                    (rng, base_dir, repo)
                },
                |(rng, _base_dir, repo)| {
                    runtime.block_on(utils::write_file(
                        rng,
                        repo,
                        Utf8Path::new("file.dat"),
                        file_size as usize,
                        buffer_size,
                    ))
                },
                BatchSize::LargeInput,
            );
        },
    );
    group.finish();
}

fn read_file(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let file_size = 1024 * 1024;
    let buffer_size = 4096;

    let mut group = c.benchmark_group("lib/read_file");
    group.sample_size(50);
    group.throughput(Throughput::Bytes(file_size));
    group.bench_function(
        BenchmarkId::from_parameter(format!("{}@{}", file_size, buffer_size)),
        |b| {
            let file_name = Utf8Path::new("file.dat");

            b.iter_batched_ref(
                || {
                    let mut rng = StdRng::from_entropy();
                    let base_dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();

                    let repo = runtime.block_on(async {
                        let repo =
                            utils::create_repo(&mut rng, &base_dir.path().join("repo.db")).await;

                        utils::write_file(
                            &mut rng,
                            &repo,
                            file_name,
                            file_size as usize,
                            buffer_size,
                        )
                        .await;
                        repo
                    });

                    (base_dir, repo)
                },
                |(_base_dir, repo)| {
                    let _len = runtime.block_on(utils::read_file(repo, file_name, buffer_size));
                },
                BatchSize::LargeInput,
            );
        },
    );
    group.finish();
}
