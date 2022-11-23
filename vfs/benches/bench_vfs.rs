use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use ouisync_lib::{AccessSecrets, MasterSecret, Repository};
use ouisync_vfs::MountGuard;
use rand::{rngs::StdRng, Rng, SeedableRng};
use std::path::Path;
use tempfile::TempDir;
use tokio::runtime::{Handle, Runtime};

criterion_group!(default, write_file);
criterion_main!(default);

fn write_file(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let file_size = 1024 * 1024;

    let mut group = c.benchmark_group("vfs/write_file");
    group.sample_size(50);
    group.throughput(Throughput::Bytes(file_size));
    group.bench_function(BenchmarkId::from_parameter(file_size), |b| {
        b.iter_batched_ref(
            || runtime.block_on(utils::setup()),
            |(rng, base_dir, _mount_guard)| {
                let file_path = base_dir.path().join("mnt").join("file.dat");
                utils::write_file(rng, &file_path, file_size);
            },
            BatchSize::LargeInput,
        );
    });
    group.finish();
}

mod utils {
    use super::*;
    use std::{
        fs::File,
        io::{self, Read},
    };

    pub async fn setup() -> (StdRng, TempDir, MountGuard) {
        let mut rng = StdRng::from_entropy();
        let base_dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let mount_dir = base_dir.path().join("mnt");

        tokio::fs::create_dir_all(&mount_dir).await.unwrap();

        let repo = Repository::create(
            base_dir.path().join("repo.db"),
            rng.gen(),
            MasterSecret::generate(&mut rng),
            AccessSecrets::generate_write(&mut rng),
            true,
        )
        .await
        .unwrap();

        let mount_guard = ouisync_vfs::mount(Handle::current(), repo, mount_dir).unwrap();

        (rng, base_dir, mount_guard)
    }

    pub fn write_file(rng: &mut StdRng, path: &Path, size: u64) {
        let mut src = RngRead(rng).take(size);
        let mut dst = File::create(path).unwrap();

        io::copy(&mut src, &mut dst).unwrap();
    }

    struct RngRead<'a>(&'a mut StdRng);

    impl Read for RngRead<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.0.fill(buffer);
            Ok(buffer.len())
        }
    }
}
