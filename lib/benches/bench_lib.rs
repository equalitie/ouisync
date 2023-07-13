mod utils;

use camino::Utf8Path;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use ouisync::StateMonitor;
use rand::{rngs::StdRng, SeedableRng};
use tempfile::TempDir;
use tokio::runtime::Runtime;
use utils::Actor;

criterion_group!(default, write_file, read_file, remove_file, sync);
criterion_main!(default);

const FILE_SIZES_K: &[u64] = &[128, 256, 512, 1024, 2 * 1024, 4 * 1024, 8 * 1024];

fn write_file(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let mut group = c.benchmark_group("lib/write_file");
    group.sample_size(10);

    let buffer_size = 4096;

    for &m in FILE_SIZES_K {
        let file_size = m * 1024;

        group.throughput(Throughput::Bytes(file_size));
        group.bench_function(BenchmarkId::from_parameter(format!("{m} kiB")), |b| {
            b.iter_batched_ref(
                || {
                    let mut rng = StdRng::from_entropy();
                    let base_dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
                    let repo = runtime.block_on(utils::create_repo(
                        &mut rng,
                        &base_dir.path().join("repo.db"),
                        0,
                        StateMonitor::make_root(),
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
        });
    }
    group.finish();
}

fn read_file(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let mut group = c.benchmark_group("lib/read_file");
    group.sample_size(10);

    let buffer_size = 4096;

    for &m in FILE_SIZES_K {
        let file_size = m * 1024;

        group.throughput(Throughput::Bytes(file_size));
        group.bench_function(BenchmarkId::from_parameter(format!("{m} kiB")), |b| {
            let file_name = Utf8Path::new("file.dat");

            b.iter_batched_ref(
                || {
                    let mut rng = StdRng::from_entropy();
                    let base_dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();

                    let repo = runtime.block_on(async {
                        let repo = utils::create_repo(
                            &mut rng,
                            &base_dir.path().join("repo.db"),
                            0,
                            StateMonitor::make_root(),
                        )
                        .await;

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
        });
    }
    group.finish();
}

fn remove_file(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let mut group = c.benchmark_group("lib/remove_file");
    group.sample_size(10);

    for &m in FILE_SIZES_K {
        let file_size = m * 1024;

        group.throughput(Throughput::Bytes(file_size));
        group.bench_function(BenchmarkId::from_parameter(format!("{m} KiB")), |b| {
            let file_name = Utf8Path::new("file.dat");

            b.iter_batched_ref(
                || {
                    let mut rng = StdRng::from_entropy();
                    let base_dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();

                    let repo = runtime.block_on(async {
                        let repo = utils::create_repo(
                            &mut rng,
                            &base_dir.path().join("repo.db"),
                            0,
                            StateMonitor::make_root(),
                        )
                        .await;
                        utils::write_file(&mut rng, &repo, file_name, file_size as usize, 4096)
                            .await;
                        repo
                    });

                    (base_dir, repo)
                },
                |(_base_dir, repo)| {
                    runtime.block_on(async { repo.remove_entry(file_name).await.unwrap() });
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn sync(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let mut group = c.benchmark_group("lib/sync");
    group.sample_size(10);

    for &m in FILE_SIZES_K {
        let file_size = m * 1024;

        group.throughput(Throughput::Bytes(file_size));
        group.bench_function(BenchmarkId::from_parameter(format!("{} kiB", m)), |b| {
            let file_name = Utf8Path::new("file.dat");

            b.iter_batched_ref(
                || {
                    let mut rng = StdRng::from_entropy();
                    let base_dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();

                    let (reader, writer) = runtime.block_on(async {
                        let reader = Actor::new(&mut rng, &base_dir.path().join("reader")).await;
                        let writer = Actor::new(&mut rng, &base_dir.path().join("writer")).await;

                        utils::write_file(
                            &mut rng,
                            &writer.repo,
                            file_name,
                            file_size as usize,
                            4096,
                        )
                        .await;

                        reader.connect_to(&writer);

                        (reader, writer)
                    });

                    (base_dir, reader, writer)
                },
                |(_base_dir, reader, writer)| {
                    runtime.block_on(utils::wait_for_sync(&reader.repo, &writer.repo));
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}
