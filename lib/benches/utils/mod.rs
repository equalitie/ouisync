use camino::Utf8Path;
use ouisync::{Access, Repository, RepositoryDb, StateMonitor, WriteSecrets};
use rand::{rngs::StdRng, Rng};
use std::{ops::Deref, path::Path};
use tokio::runtime::Handle;

pub async fn create_repo(rng: &mut StdRng, store: &Path) -> RepositoryGuard {
    let monitor = StateMonitor::make_root();
    let repository = Repository::create(
        RepositoryDb::create(store, monitor).await.unwrap(),
        rng.gen(),
        Access::WriteUnlocked {
            secrets: WriteSecrets::generate(rng),
        },
    )
    .await
    .unwrap();

    RepositoryGuard {
        repository,
        handle: Handle::current(),
    }
}

// Wrapper for `Repository` which calls `close` on drop.
pub struct RepositoryGuard {
    repository: Repository,
    handle: Handle,
}

impl Deref for RepositoryGuard {
    type Target = Repository;

    fn deref(&self) -> &Self::Target {
        &self.repository
    }
}

impl Drop for RepositoryGuard {
    fn drop(&mut self) {
        self.handle
            .block_on(async { self.repository.close().await.unwrap() })
    }
}

/// Write `size` random bytes to a file at `path` (`buffer_size` bytes at a time).
pub async fn write_file(
    rng: &mut StdRng,
    repo: &Repository,
    path: &Utf8Path,
    size: usize,
    buffer_size: usize,
) {
    let mut file = repo.create_file(path).await.unwrap();

    if size == 0 {
        return;
    }

    let mut remaining = size;
    let mut buffer = vec![0; buffer_size];

    while remaining > 0 {
        let len = buffer_size.min(remaining);

        rng.fill(&mut buffer[..len]);
        file.write(&buffer[..len]).await.unwrap();

        remaining -= len;
    }

    file.flush().await.unwrap();
}

/// Read the whole content of the file at `path` in `buffer_size` bytes at a time. Returns the
/// total size of the content.
pub async fn read_file(repo: &Repository, path: &Utf8Path, buffer_size: usize) -> usize {
    let mut file = repo.open_file(path).await.unwrap();
    let mut buffer = vec![0; buffer_size];
    let mut size = 0;

    loop {
        let len = file.read(&mut buffer[..]).await.unwrap();

        if len == 0 {
            break;
        }

        size += len;
    }

    size
}
