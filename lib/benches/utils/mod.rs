use camino::Utf8Path;
use ouisync::{AccessSecrets, MasterSecret, Repository};
use rand::{rngs::StdRng, Rng};
use std::path::Path;

pub async fn create_repo(rng: &mut StdRng, store: &Path) -> Repository {
    Repository::create(
        store,
        rng.gen(),
        MasterSecret::generate(rng),
        AccessSecrets::generate_write(rng),
        true,
    )
    .await
    .unwrap()
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

    let mut conn = repo.db().acquire().await.unwrap();
    let mut remaining = size;
    let mut buffer = vec![0; buffer_size];

    while remaining > 0 {
        let len = buffer_size.min(remaining);

        rng.fill(&mut buffer[..len]);
        file.write(&mut conn, &buffer[..len]).await.unwrap();

        remaining -= len;
    }

    file.flush(&mut conn).await.unwrap();
}

/// Read the whole content of the file at `path` in `buffer_size` bytes at a time. Returns the
/// total size of the content.
pub async fn read_file(repo: &Repository, path: &Utf8Path, buffer_size: usize) -> usize {
    let mut file = repo.open_file(path).await.unwrap();
    let mut conn = repo.db().acquire().await.unwrap();
    let mut buffer = vec![0; buffer_size];
    let mut size = 0;

    loop {
        let len = file.read(&mut conn, &mut buffer[..]).await.unwrap();

        if len == 0 {
            break;
        }

        size += len;
    }

    size
}
