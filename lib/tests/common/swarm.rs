use super::{Env, Proto, actor};
use camino::{Utf8Path, Utf8PathBuf};
use ouisync::{Error, Repository, StoreError};
use rand::RngExt;
use std::{
    cmp::Ordering,
    collections::HashSet,
    io::{Read, SeekFrom},
    ops::{Range, RangeInclusive},
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{Barrier, broadcast, mpsc},
    time::sleep,
};
use tracing::{Instrument, instrument};

pub(crate) struct SyncSwarmCase {
    pub min_peers: usize,
    pub max_peers: usize,
    pub num_repos: usize,
    pub min_files_per_repo: usize,
    pub max_files_per_repo: usize,
    pub min_file_size: usize,
    pub max_file_size: usize,
}

impl Default for SyncSwarmCase {
    fn default() -> Self {
        Self {
            min_peers: 2,
            max_peers: 2,
            num_repos: 1,
            min_files_per_repo: 1,
            max_files_per_repo: 1,
            min_file_size: 1024,
            max_file_size: 1024,
        }
    }
}

impl SyncSwarmCase {
    pub fn num_peers(self, value: impl RandomUsize) -> Self {
        let (min, max) = value.into_range();
        Self {
            min_peers: min,
            max_peers: max,
            ..self
        }
    }

    pub fn num_repos(self, value: usize) -> Self {
        Self {
            num_repos: value,
            ..self
        }
    }

    pub fn num_files_per_repo(self, value: impl RandomUsize) -> Self {
        let (min, max) = value.into_range();
        Self {
            min_files_per_repo: min,
            max_files_per_repo: max,
            ..self
        }
    }

    pub fn file_size(self, value: impl RandomUsize) -> Self {
        let (min, max) = value.into_range();
        Self {
            min_file_size: min,
            max_file_size: max,
            ..self
        }
    }

    pub fn run(&self) {
        use ouisync_rand_test_dir as rtd;

        assert!(self.min_peers > 1);
        assert!(self.min_peers <= self.max_peers);
        assert!(self.num_repos > 0);

        let mut env = Env::new();
        let mut rng = rand::rng();

        let num_peers = rng.random_range(self.min_peers..=self.max_peers);
        let barrier = Arc::new(Barrier::new(num_peers));

        let contents: Vec<_> = (0..self.num_repos)
            .map(|_| {
                rtd::Directory::generate(
                    &mut rng,
                    rtd::GenerateOptions {
                        min_files: self.min_files_per_repo,
                        max_files: self.max_files_per_repo,
                        min_file_size: self.min_file_size,
                        max_file_size: self.max_file_size,
                        ..Default::default()
                    },
                )
            })
            .collect();

        for actor_index in 0..num_peers {
            env.actor(&format!("actor-{actor_index}"), {
                let contents = contents.clone();
                let barrier = barrier.clone();

                async move {
                    let network = actor::create_network(Proto::Tcp).await;

                    // Connect to the others
                    for other_actor_index in 0..num_peers {
                        if other_actor_index == actor_index {
                            continue;
                        }

                        network.add_user_provided_peer(
                            &actor::lookup_addr(&format!("actor-{other_actor_index}")).await,
                        );
                    }

                    // Create repos and files
                    let mut repos = Vec::with_capacity(contents.len());
                    for repo_index in 0..contents.len() {
                        repos.push(
                            actor::create_linked_repo(&format!("repo-{repo_index}"), &network)
                                .await,
                        );
                    }

                    for (repo_index, (repo, _)) in repos.iter().enumerate() {
                        // Try to create each file by different actor but if there is more files than
                        // actors then some actors create more than one file.
                        for (path, test_file) in walk_files(&contents[repo_index])
                            .skip(actor_index)
                            .step_by(num_peers)
                        {
                            async {
                                let path = Utf8Path::new(&path);
                                if let Some(parent) = path.parent() {
                                    repo.create_directory(parent).await.unwrap();
                                }
                                let mut file = repo.create_file(path).await.unwrap();
                                write(test_file, &mut file).await;
                                file.flush().await.unwrap();
                            }
                            .instrument(info_span!("write", repo = repo_index, file = %path))
                            .await
                        }
                    }

                    for (repo_index, content) in contents.iter().enumerate() {
                        let repo = &repos[repo_index].0;

                        for (path, test_file) in walk_files(content) {
                            check(repo, &path, test_file)
                                .instrument(info_span!("read", repo = repo_index, file = %path))
                                .await;
                        }
                    }

                    barrier.wait().await;
                }
            });
        }

        fn walk_files(root: &rtd::Directory) -> impl Iterator<Item = (Utf8PathBuf, &rtd::File)> {
            root.walk()
                .filter_map(|(path, entry)| match entry {
                    rtd::Entry::File(file) => Some((path, file)),
                    rtd::Entry::Directory(_) => None,
                })
                .map(|(path, file)| (Utf8PathBuf::from(path), file))
        }

        // Write the content of `dst` into `src`.
        async fn write(src: &rtd::File, dst: &mut ouisync::File) {
            use std::io::Read;

            let mut buffer = vec![0; 8 * 1024];
            let mut reader = src.reader();

            loop {
                // NOTE: this `read` never blocks so it's ok to use it in an `async` function.
                let n = reader.read(&mut buffer).unwrap();
                if n == 0 {
                    break;
                }

                dst.write_all(&buffer[..n]).await.unwrap();
            }
        }

        // Wait until the file at `path` in `repo` has the same content as `expected`.
        async fn check(repo: &Repository, path: &Utf8Path, expected: &rtd::File) {
            // By specifying local branch we ensure forking has finished successfully.
            // This makes the stats printed at the end of the test more deterministic
            // because without it sometimes the fork would not finish and thus wouldn't
            // contribute to the stats. This was observed mainly in the two node test.
            let branch_id = *repo.local_branch().unwrap().id();

            super::eventually(repo, async || {
                let Some(mut actual) =
                    super::open_file_version(repo, path.as_str(), Some(&branch_id)).await
                else {
                    return false;
                };

                // For large files this is faster than reading the file content and compare.
                if actual.len() != expected.len() as u64 {
                    return false;
                }

                let mut actual_buffer = vec![0; 1024];
                let mut expected_reader = expected.reader();
                let mut expected_buffer = vec![0; 1024];

                loop {
                    let n = match actual.read(&mut actual_buffer).await {
                        Ok(0) => {
                            debug!(%path, "content matches");
                            break true;
                        }
                        Ok(n) => n,
                        // `BlockNotFound` means just that some block of the file hasn't been
                        // downloaded yet. `LocatorNotFound` likely means we've received a partially
                        // merged branch and a block referenced by a blob is not yet referenced by
                        // that branch.
                        Err(
                            error @ (Error::Store(StoreError::BlockNotFound)
                            | Error::Store(StoreError::LocatorNotFound)),
                        ) => {
                            warn!(%path, ?error, "read failed");
                            break false;
                        }
                        Err(error) => {
                            error!(%path, ?error);
                            panic!("unexpected error: {error:?}");
                        }
                    };

                    expected_reader
                        .read_exact(&mut expected_buffer[..n])
                        .unwrap();

                    if actual_buffer[..n] != expected_buffer[..n] {
                        warn!(%path, "content does not match");
                        break false;
                    }
                }
            })
            .await;
        }
    }
}

/// Trait for types that represent random usize value chosen uniformly from some range.
pub(crate) trait RandomUsize {
    fn into_range(self) -> (usize, usize);
}

impl RandomUsize for usize {
    fn into_range(self) -> (usize, usize) {
        (self, self)
    }
}

impl RandomUsize for Range<usize> {
    fn into_range(self) -> (usize, usize) {
        (self.start, self.end - 1)
    }
}

impl RandomUsize for RangeInclusive<usize> {
    fn into_range(self) -> (usize, usize) {
        (*self.start(), *self.end())
    }
}
