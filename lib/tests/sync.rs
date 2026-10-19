//! Synchronization tests

#[macro_use]
mod common;

use self::common::{DEFAULT_REPO, Env, Proto, actor, dump, sync_watch};
use assert_matches::assert_matches;
use backoff::{ExponentialBackoffBuilder, backoff::Backoff};
use camino::{Utf8Path, Utf8PathBuf};
use ouisync::{
    Access, AccessMode, BLOB_HEADER_SIZE, BLOCK_SIZE, EntryType, Error, Payload, Repository,
    StorageSize, StoreError, VersionVector,
};
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

const SMALL_SIZE: usize = 1024;
const LARGE_SIZE: usize = 2 * 1024 * 1024;
const HUGE_SIZE: usize = 30 * 1024 * 1024;

#[test]
fn sync_two_peers_one_repo_small() {
    SyncSwarmCase::default()
        .num_peers(2)
        .num_repos(1)
        .num_files_per_repo(1)
        .file_size(SMALL_SIZE)
        .run();
}

#[test]
fn sync_three_peers_one_repo_small() {
    SyncSwarmCase::default()
        .num_peers(3)
        .num_repos(1)
        .num_files_per_repo(1)
        .file_size(SMALL_SIZE)
        .run()
}

#[test]
fn sync_two_peers_one_repo_large() {
    SyncSwarmCase::default()
        .num_peers(2)
        .num_repos(1)
        .num_files_per_repo(1)
        .file_size(LARGE_SIZE)
        .run()
}

#[ignore]
#[test]
fn sync_two_peers_one_repo_huge() {
    SyncSwarmCase::default()
        .num_peers(2)
        .num_repos(1)
        .num_files_per_repo(1)
        .file_size(HUGE_SIZE)
        .run()
}

#[test]
fn sync_three_peers_one_repo_large() {
    SyncSwarmCase::default()
        .num_peers(3)
        .num_repos(1)
        .num_files_per_repo(1)
        .file_size(LARGE_SIZE)
        .run()
}

#[ignore]
#[test]
fn sync_three_peers_one_repo_huge() {
    SyncSwarmCase::default()
        .num_peers(3)
        .num_repos(1)
        .num_files_per_repo(1)
        .file_size(HUGE_SIZE)
        .run()
}

#[test]
fn sync_two_peers_two_repos_small() {
    SyncSwarmCase::default()
        .num_peers(2)
        .num_repos(2)
        .num_files_per_repo(1)
        .file_size(SMALL_SIZE)
        .run()
}

#[test]
fn sync_two_peers_two_repos_large() {
    SyncSwarmCase::default()
        .num_peers(2)
        .num_repos(2)
        .num_files_per_repo(1)
        .file_size(LARGE_SIZE)
        .run()
}

#[test]
fn sync_many_peers_one_repo_large() {
    SyncSwarmCase::default()
        .num_peers(10)
        .num_repos(1)
        .num_files_per_repo(1)
        .file_size(LARGE_SIZE)
        .run()
}

struct SyncSwarmCase {
    num_peers: usize,
    num_repos: usize,
    min_files_per_repo: usize,
    max_files_per_repo: usize,
    min_file_size: usize,
    max_file_size: usize,
}

impl Default for SyncSwarmCase {
    fn default() -> Self {
        Self {
            num_peers: 2,
            num_repos: 1,
            min_files_per_repo: 1,
            max_files_per_repo: 1,
            min_file_size: 1024,
            max_file_size: 1024,
        }
    }
}

impl SyncSwarmCase {
    fn num_peers(self, value: usize) -> Self {
        Self {
            num_peers: value,
            ..self
        }
    }

    fn num_repos(self, value: usize) -> Self {
        Self {
            num_repos: value,
            ..self
        }
    }

    fn num_files_per_repo(self, value: impl RandomUsize) -> Self {
        let (min, max) = value.into_range();
        Self {
            min_files_per_repo: min,
            max_files_per_repo: max,
            ..self
        }
    }

    fn file_size(self, value: impl RandomUsize) -> Self {
        let (min, max) = value.into_range();
        Self {
            min_file_size: min,
            max_file_size: max,
            ..self
        }
    }

    fn run(&self) {
        use ouisync_rand_test_dir as rtd;

        assert!(self.num_peers > 1);
        assert!(self.num_repos > 0);

        let mut env = Env::new();
        let barrier = Arc::new(Barrier::new(self.num_peers));

        let mut rng = rand::rng();
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

        for actor_index in 0..self.num_peers {
            env.actor(&format!("actor-{actor_index}"), {
                let num_peers = self.num_peers;
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

        fn walk_files<'a>(
            root: &'a rtd::Directory,
        ) -> impl Iterator<Item = (Utf8PathBuf, &'a rtd::File)> {
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

            common::eventually(repo, async || {
                let Some(mut actual) =
                    common::open_file_version(repo, path.as_str(), Some(&branch_id)).await
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
trait RandomUsize {
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

#[test]
fn sync_directory_with_file() {
    sync_dump_case(
        dump::Directory::new().add("food", dump::Directory::new().add("pizza.jpg", vec![])),
    );
}

#[test]
fn sync_directory_with_subdirectory() {
    sync_dump_case(dump::Directory::new().add(
        "food",
        dump::Directory::new().add("mediterranean", dump::Directory::new()),
    ));
}

#[test]
fn sync_two_directories_one_with_subdirectory() {
    sync_dump_case(
        dump::Directory::new()
            .add("dir-a", dump::Directory::new())
            .add(
                "dir-b",
                dump::Directory::new().add("subdir", dump::Directory::new()),
            ),
    );
}

#[test]
fn sync_many_files() {
    let mut rng = rand::rng();

    let num_files = 10;
    let min_size = 0;
    let max_size = 128 * 1024;

    let dump = (0..num_files)
        .map(|index| {
            let name = format!("file-{index}.dat");
            let size = rng.random_range(min_size..max_size);
            let content = common::random_bytes(size);

            (name, content)
        })
        .fold(dump::Directory::new(), |dump, (name, content)| {
            dump.add(name, content)
        });

    sync_dump_case(dump);
}

fn sync_dump_case(dump: dump::Directory) {
    let mut env = Env::new();
    let (tx, rx) = sync_watch::channel();
    let dump = Arc::new(dump);

    env.actor("writer", {
        let dump = dump.clone();
        async move {
            let (_network, repo, _reg) = actor::setup().await;

            dump::load(&repo, &dump).await;

            info!("dump load complete");

            tx.run(&repo).await;
        }
    });

    env.actor("reader", {
        async move {
            let (network, repo, _reg) = actor::setup().await;
            network.add_user_provided_peer(&actor::lookup_addr("writer").await);

            rx.run(&repo).await;

            let actual_dump = dump::save(&repo).await;
            similar_asserts::assert_eq!(actual_dump, *dump);
        }
    });
}

#[test]
fn relink_repository() {
    let mut env = Env::new();

    // Bidirectional side-channel
    let (writer_tx, mut writer_rx) = mpsc::channel(1);
    let (reader_tx, mut reader_rx) = mpsc::channel(1);

    env.actor("writer", async move {
        let (_network, repo, _reg) = actor::setup().await;

        // Create the file
        let mut file = repo.create_file("test.txt").await.unwrap();
        file.write_all(b"first").await.unwrap();
        file.flush().await.unwrap();

        // Wait for the reader to see the file and then unlink its repo
        reader_rx.recv().await;

        // Modify the file while reader is unlinked
        file.truncate(0).unwrap();
        file.write_all(b"second").await.unwrap();
        file.flush().await.unwrap();

        // Notify the reader the file is updated
        writer_tx.send(()).await.unwrap();

        // Wait until reader is done
        reader_rx.recv().await;
    });

    env.actor("reader", async move {
        let (network, repo, reg) = actor::setup().await;

        let peer_addr = actor::lookup_addr("writer").await;
        network.add_user_provided_peer(&peer_addr);

        // Wait until we see the original file
        common::expect_file_content(&repo, "test.txt", b"first").await;

        // Unlink the repo and notify the writer
        drop(reg);
        reader_tx.send(()).await.unwrap();

        // Wait until writer is done updating the file
        writer_rx.recv().await;

        // Relink the repo
        let _reg = network.register(repo.handle());

        // Wait until the file is updated
        common::expect_file_content(&repo, "test.txt", b"second").await;

        // Notify the writer that we are done
        reader_tx.send(()).await.unwrap();
    });
}

#[test]
fn remove_remote_file() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    env.actor("creator", async move {
        let (_network, repo, _reg) = actor::setup().await;

        let mut file = repo.create_file("test.txt").await.unwrap();
        file.flush().await.unwrap();
        drop(file);

        common::expect_entry_not_found(&repo, "test.txt").await;

        tx.send(()).await.unwrap();
    });

    env.actor("remover", async move {
        let (network, repo, _reg) = actor::setup().await;

        let peer_addr = actor::lookup_addr("creator").await;
        network.add_user_provided_peer(&peer_addr);

        common::expect_file_content(&repo, "test.txt", &[]).await;

        repo.remove_entry("test.txt").await.unwrap();

        rx.recv().await;
    });
}

#[test]
fn relay_write() {
    let file_size = LARGE_SIZE;
    relay_case(Proto::Tcp, file_size, AccessMode::Write)
}

#[test]
fn relay_blind() {
    let file_size = LARGE_SIZE;
    relay_case(Proto::Tcp, file_size, AccessMode::Blind)
}

// Simulate two peers that can't connect to each other but both can connect to a third ("relay")
// peer.
fn relay_case(proto: Proto, file_size: usize, relay_access_mode: AccessMode) {
    let mut env = Env::new();
    let (tx, _) = broadcast::channel(1);

    let content = Arc::new(common::random_bytes(file_size));

    env.actor("relay", {
        let mut rx = tx.subscribe();

        async move {
            let network = actor::create_network(proto).await;
            let repo = actor::create_repo_with_mode(DEFAULT_REPO, relay_access_mode).await;
            let _reg = network.register(repo.handle());

            rx.recv().await.unwrap();
        }
    });

    env.actor("writer", {
        let content = content.clone();
        let mut rx = tx.subscribe();

        async move {
            let (network, repo, _reg) = actor::setup().await;
            network.add_user_provided_peer(&actor::lookup_addr("relay").await);

            let mut file = repo.create_file("test.dat").await.unwrap();
            common::write_in_chunks(&mut file, &content, 4096).await;
            file.flush().await.unwrap();

            rx.recv().await.unwrap();
        }
    });

    env.actor("reader", {
        async move {
            let (network, repo, _reg) = actor::setup().await;
            network.add_user_provided_peer(&actor::lookup_addr("relay").await);

            common::expect_file_content(&repo, "test.dat", &content).await;

            tx.send(()).unwrap();
        }
    });
}

// Test for an edge case where a sync happens while we are in the middle of writing a file.
// This test makes sure that when the sync happens, the partially written file content is not
// garbage collected prematurelly.
#[test]
fn sync_during_file_write() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    let content = common::random_bytes(3 * BLOCK_SIZE - BLOB_HEADER_SIZE);

    env.actor("alice", {
        let content = content.clone();

        async move {
            let (_network, repo, _reg) = actor::setup().await;

            // Create empty file and wait until bob sees it
            let mut file = repo.create_file("foo.txt").await.unwrap();
            rx.recv().await;

            // Write half of the file content but don't flush yet.
            common::write_in_chunks(&mut file, &content[..content.len() / 2], 4096)
                .instrument(info_span!("write", file = "foo.txt", step = 1))
                .await;

            // Wait until we see the file created by B
            common::expect_file_content(&repo, "bar.txt", b"bar").await;

            // Write the second half of the content and flush.
            async {
                common::write_in_chunks(&mut file, &content[content.len() / 2..], 4096).await;
                file.flush().await.unwrap();
            }
            .instrument(info_span!("write", file = "foo.txt", step = 2))
            .await;

            // Reopen the file and verify it has the expected full content
            let mut file = repo.open_file("foo.txt").await.unwrap();
            let actual_content = file
                .read_to_end()
                .instrument(info_span!("read", file = "foo.txt"))
                .await
                .unwrap();
            similar_asserts::assert_eq!(&actual_content, &content);

            rx.recv().await;
        }
    });

    env.actor("bob", {
        async move {
            let (network, repo, _reg) = actor::setup().await;
            network.add_user_provided_peer(&actor::lookup_addr("alice").await);

            // Wait until the file gets merged
            common::expect_local_file_content(&repo, "foo.txt", &[]).await;
            tx.send(()).await.unwrap();

            // Write a file. Excluding the unflushed changes by Alice, this makes Bob's branch newer
            // than Alice's.
            async {
                let mut file = repo.create_file("bar.txt").await.unwrap();
                file.write_all(b"bar").await.unwrap();
                file.flush().await.unwrap();
            }
            .instrument(info_span!("write", file = "bar.txt"))
            .await;

            // Wait until we see the file with the complete content from Alice
            common::expect_file_content(&repo, "foo.txt", &content).await;
            tx.send(()).await.unwrap();
        }
    });
}

#[test]
fn concurrent_modify_open_file() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    let content_a = Arc::new(common::random_bytes(2 * BLOCK_SIZE - BLOB_HEADER_SIZE));
    let content_b = Arc::new(common::random_bytes(2 * BLOCK_SIZE - BLOB_HEADER_SIZE));

    env.actor("alice", {
        let content_b = content_b.clone();

        async move {
            let (_network, repo, _reg) = actor::setup().await;

            // Create empty file and wait until Bob sees it
            let mut file = repo.create_file("file.txt").await.unwrap();

            let id_a = *repo.local_branch().unwrap().id();
            let id_b = rx.recv().await.unwrap();

            // Write to file but don't flush yet
            common::write_in_chunks(&mut file, &content_a, 4096).await;

            // Wait until we see Bob's writes
            common::expect_file_version_content(&repo, "file.txt", Some(&id_b), &content_b).await;

            // A: Flush the file
            file.flush().await.unwrap();
            drop(file);

            // Verify both versions of the file are still present
            assert_matches!(repo.open_file("file.txt").await, Err(Error::AmbiguousEntry));

            let mut file_a = repo.open_file_version("file.txt", &id_a).await.unwrap();
            let actual_content_a = file_a.read_to_end().await.unwrap();

            let mut file_b = repo.open_file_version("file.txt", &id_b).await.unwrap();
            let actual_content_b = file_b.read_to_end().await.unwrap();

            assert!(actual_content_a == *content_a);
            assert!(actual_content_b == *content_b);
        }
    });

    env.actor("bob", {
        async move {
            let (network, repo, _reg) = actor::setup().await;
            network.add_user_provided_peer(&actor::lookup_addr("alice").await);

            let id_b = *repo.local_branch().unwrap().id();

            // Wait until the file gets merged
            common::expect_file_version_content(&repo, "file.txt", Some(&id_b), &[]).await;
            tx.send(id_b).await.unwrap();

            // Write to the same file and flush
            let mut file = repo.open_file("file.txt").await.unwrap();
            common::write_in_chunks(&mut file, &content_b, 4096).await;
            file.flush().await.unwrap();

            tx.closed().await;
        }
    });
}

// Test that the local version changes monotonically even when the local branch temporarily becomes
// outdated.
// TODO: This test is too low level. Consider converting it into unit test.
#[ignore] // FIXME
#[test]
fn recreate_local_branch() {
    let mut env = Env::new();
    let proto = Proto::Tcp;
    let (tx, mut rx) = mpsc::channel(1);

    env.actor("alice", async move {
        let network = actor::create_network(proto).await;

        // 1. Create the repo but don't link it yet.
        let params = actor::get_repo_params(DEFAULT_REPO);
        let secrets = actor::get_repo_secrets(DEFAULT_REPO);
        let repo = Repository::create(&params, Access::new(None, None, secrets))
            .await
            .unwrap();

        // 2. Create a file
        let mut file = repo.create_file("foo.txt").await.unwrap();
        file.write_all(b"hello from Alice\n").await.unwrap();
        file.flush().await.unwrap();
        drop(file);

        let vv_a_0 = repo.local_branch().unwrap().version_vector().await.unwrap();

        // 3. Reopen the repo in read mode to disable merger
        repo.set_access_mode(AccessMode::Read, None).await.unwrap();

        // 4. Establish link
        let reg = network.register(repo.handle());

        // 7. Sync with Bob. Afterwards our local branch will become outdated compared to Bob's
        common::expect_file_content(&repo, "foo.txt", b"hello from Alice\nhello from Bob\n").await;

        // 8: Reopen in write mode
        drop(reg);

        repo.set_access_mode(AccessMode::Write, None).await.unwrap();

        // 9. Modify the repo
        repo.create_file("bar.txt").await.unwrap();

        // 10. Make sure the local version changed monotonically.
        let vv_b: VersionVector = rx.recv().await.unwrap();
        assert_eq!(
            vv_b.partial_cmp(&vv_a_0),
            Some(Ordering::Greater),
            "expected {vv_b:?} > {vv_a_0:?}"
        );

        common::eventually(&repo, || async {
            let vv_a_1 = repo.local_branch().unwrap().version_vector().await.unwrap();
            vv_a_1 > vv_b
        })
        .await;
    });

    env.actor("bob", async move {
        let (network, repo, _reg) = actor::setup().await;
        network.add_user_provided_peer(&actor::lookup_addr("alice").await);

        // 5. Sync with Alice
        common::expect_local_file_content(&repo, "foo.txt", b"hello from Alice\n").await;

        // 6. Modify the repo. This makes Bob's branch newer than Alice's
        let mut file = repo.open_file("foo.txt").await.unwrap();
        file.seek(SeekFrom::End(0));
        file.write_all(b"hello from Bob\n").await.unwrap();
        file.flush().await.unwrap();
        drop(file);

        let vv_b = repo.local_branch().unwrap().version_vector().await.unwrap();
        tx.send(vv_b).await.unwrap();
        tx.closed().await;
    });
}

#[test]
fn remote_rename_file() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    env.actor("writer", async move {
        let (_network, repo, _reg) = actor::setup().await;

        // Create the file and wait until reader has seen it
        repo.create_file("foo.txt").await.unwrap();
        rx.recv().await;

        // Rename it and wait until reader is done
        repo.move_entry("foo.txt", "bar.txt").await.unwrap();
        rx.recv().await;
    });

    env.actor("reader", async move {
        let (network, repo, _reg) = actor::setup().await;
        network.add_user_provided_peer(&actor::lookup_addr("writer").await);

        common::expect_entry_exists(&repo, "foo.txt", EntryType::File).await;
        tx.send(()).await.unwrap();

        common::expect_entry_exists(&repo, "bar.txt", EntryType::File).await;
        common::expect_entry_not_found(&repo, "foo.txt").await;
        tx.send(()).await.unwrap();
    });
}

#[test]
fn remote_rename_empty_directory() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    env.actor("writer", async move {
        let (_network, repo, _reg) = actor::setup().await;

        // Create directory and wait until reader has seen it
        repo.create_directory("foo").await.unwrap();
        rx.recv().await;

        // Rename the directory and wait until reader is done
        repo.move_entry("foo", "bar").await.unwrap();
        rx.recv().await;
    });

    env.actor("reader", async move {
        let (network, repo, _reg) = actor::setup().await;
        network.add_user_provided_peer(&actor::lookup_addr("writer").await);

        common::expect_entry_exists(&repo, "foo", EntryType::Directory).await;
        tx.send(()).await.unwrap();

        common::expect_entry_exists(&repo, "bar", EntryType::Directory).await;
        common::expect_entry_not_found(&repo, "foo").await;
        tx.send(()).await.unwrap();
    });
}

#[test]
fn remote_rename_non_empty_directory() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    env.actor("writer", async move {
        let (_network, repo, _reg) = actor::setup().await;

        // Create a directory with content and wait until reader has seen it
        let mut dir = repo.create_directory("foo").await.unwrap();
        dir.create_file("data.txt".into()).await.unwrap();
        rx.recv().await;

        // Drop the dir otherwise the subsequent `move_entry` fails with `Error::Locked`.
        drop(dir);

        // Rename the directory and wait until reader is done
        repo.move_entry("foo", "bar").await.unwrap();
        rx.recv().await;
    });

    env.actor("reader", async move {
        let (network, repo, _reg) = actor::setup().await;
        network.add_user_provided_peer(&actor::lookup_addr("writer").await);

        common::expect_entry_exists(&repo, "foo/data.txt", EntryType::File).await;
        tx.send(()).await.unwrap();

        common::expect_entry_exists(&repo, "bar/data.txt", EntryType::File).await;
        common::expect_entry_not_found(&repo, "foo").await;
        tx.send(()).await.unwrap();
    });
}

#[test]
fn remote_rename_directory_during_conflict() {
    let mut env = Env::new();
    let proto = Proto::Tcp;
    let (tx, mut rx) = mpsc::channel(1);

    env.actor("writer", async move {
        let network = actor::create_network(proto).await;
        let repo = actor::create_repo(DEFAULT_REPO).await;

        // Create file before linking the repo to ensure we create conflict.
        repo.create_file("dummy.txt")
            .instrument(info_span!("create", message = "dummy.txt"))
            .await
            .unwrap();

        let _reg = network.register(repo.handle());

        repo.create_directory("foo")
            .instrument(info_span!("create", message = "foo"))
            .await
            .unwrap();
        rx.recv().await;

        repo.move_entry("foo", "bar")
            .instrument(info_span!("move", src = "foo", dst = "bar"))
            .await
            .unwrap();
        rx.recv().await;
    });

    env.actor("reader", async move {
        let network = actor::create_network(proto).await;
        let repo = actor::create_repo(DEFAULT_REPO).await;

        network.add_user_provided_peer(&actor::lookup_addr("writer").await);

        // Create file before linking the repo to ensure we create conflict.
        // This prevents the remote branch from being pruned.
        repo.create_file("dummy.txt")
            .instrument(info_span!("create dummy.txt"))
            .await
            .unwrap();

        let _reg = network.register(repo.handle());

        expect_local_directory_exists(&repo, "foo").await;
        tx.send(()).await.unwrap();

        common::expect_entry_exists(&repo, "bar", EntryType::Directory).await;
        common::expect_entry_not_found(&repo, "foo").await;
        tx.send(()).await.unwrap();
    });
}

#[test]
fn remote_move_file_to_directory_then_rename_that_directory() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    env.actor("writer", async move {
        let (_network, repo, _reg) = actor::setup().await;

        repo.create_file("data.txt").await.unwrap();
        rx.recv().await;

        repo.create_directory("archive").await.unwrap();
        repo.move_entry("data.txt", "archive/data.txt")
            .await
            .unwrap();
        rx.recv().await;

        repo.move_entry("archive", "trash").await.unwrap();
        rx.recv().await;
    });

    env.actor("reader", async move {
        let (network, repo, _reg) = actor::setup().await;
        network.add_user_provided_peer(&actor::lookup_addr("writer").await);

        common::expect_entry_exists(&repo, "data.txt", EntryType::File).await;
        tx.send(()).await.unwrap();

        common::expect_entry_exists(&repo, "archive/data.txt", EntryType::File).await;
        tx.send(()).await.unwrap();

        common::expect_entry_exists(&repo, "trash/data.txt", EntryType::File).await;
        common::expect_entry_not_found(&repo, "data.txt").await;
        common::expect_entry_not_found(&repo, "archive").await;
        tx.send(()).await.unwrap();
    });
}

#[test]
fn concurrent_update_and_delete_during_conflict() {
    let mut env = Env::new();
    let proto = Proto::Tcp;

    let (alice_tx, mut alice_rx) = mpsc::channel(1);
    let (bob_tx, mut bob_rx) = mpsc::channel(1);

    // Initial file content
    let content_v0 = common::random_bytes(2 * BLOCK_SIZE - BLOB_HEADER_SIZE); // exactly 2 blocks

    // Content later edited by overwriting its end with this (shorter than 1 block so that the
    // first block remains unchanged)
    let chunk = common::random_bytes(64);

    // Expected file content after the edit
    let content_v1 = {
        let mut content = content_v0.clone();
        content[content_v0.len() - chunk.len()..].copy_from_slice(&chunk);
        content
    };

    env.actor("alice", {
        let content_v0 = content_v0.clone();

        async move {
            let network = actor::create_network(proto).await;
            let repo = actor::create_repo(DEFAULT_REPO).await;

            let id_a = *repo.local_branch().unwrap().id();
            let id_b = bob_rx.recv().await.unwrap();

            // 1a. Create a conflict so the branches stay concurrent. This prevents the remote
            // branch from being pruned.
            repo.create_file("dummy.txt").await.unwrap();

            let reg = network.register(repo.handle());

            // 3. Wait until the file gets merged
            common::expect_file_version_content(&repo, "data.txt", Some(&id_a), &content_v0).await;
            alice_tx.send(()).await.unwrap();

            // 4a. Unlink to allow concurrent operations on the file
            drop(reg);

            // 5a. Remove the file (this garbage-collects its blocks)
            repo.remove_entry("data.txt").await.unwrap();

            // 6a. Relink
            let _reg = network.register(repo.handle());

            // 7. We are able to read the whole file again including the previously gc-ed blocks.
            common::expect_file_version_content(&repo, "data.txt", Some(&id_b), &content_v1).await;

            alice_tx.send(()).await.unwrap();
        }
    });

    env.actor("bob", {
        async move {
            let network = actor::create_network(proto).await;
            network.add_user_provided_peer(&actor::lookup_addr("alice").await);

            let repo = actor::create_repo(DEFAULT_REPO).await;

            bob_tx
                .send(*repo.local_branch().unwrap().id())
                .await
                .unwrap();

            // 1b. Create a conflict so the branches stay concurrent. This prevents the remote branch
            // from being pruned.
            repo.create_file("dummy.txt").await.unwrap();

            let reg = network.register(repo.handle());

            // 2. Create the file and wait until alice sees it
            let mut file = repo.create_file("data.txt").await.unwrap();
            async {
                file.write_all(&content_v0).await.unwrap();
                file.flush().await.unwrap();
            }
            .instrument(info_span!("write", file = "data.txt", step = 1))
            .await;

            alice_rx.recv().await.unwrap();

            // 4b. Unlink to allow concurrent operations on the file
            drop(reg);

            // 5b. Writes to the file in such a way that the first block remains unchanged
            async {
                file.seek(SeekFrom::End(-(chunk.len() as i64)));
                file.write_all(&chunk).await.unwrap();
                file.flush().await.unwrap();
            }
            .instrument(info_span!("write", file = "data.txt", step = 2))
            .await;

            // 6b. Relink
            let _reg = network.register(repo.handle());

            alice_rx.recv().await.unwrap();
        }
    });
}

// https://github.com/equalitie/ouisync/issues/86
#[test]
fn content_stays_available_during_sync() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    let content0 = common::random_bytes(32);
    let content1 = common::random_bytes(128 * 1024);

    env.actor("alice", {
        let content0 = content0.clone();
        let content1 = content1.clone();

        async move {
            let (_network, repo, _reg) = actor::setup().await;

            // 1. Create "b/c.dat" and wait for Bob to see it.
            let mut file = repo.create_file("b/c.dat").await.unwrap();
            file.write_all(&content0).await.unwrap();
            file.flush().await.unwrap();

            rx.recv().await.unwrap();

            // 3. Then create "a.dat" and "b/d.dat".
            let mut file = repo.create_file("a.dat").await.unwrap();
            file.write_all(&content1).await.unwrap();
            file.flush().await.unwrap();

            repo.create_file("b/d.dat").await.unwrap();

            rx.recv().await.unwrap();
        }
    });

    env.actor("bob", {
        async move {
            // Bob is read-only to disable the merger which could otherwise interfere with this test.
            let network = actor::create_network(Proto::Tcp).await;
            let repo = actor::create_repo_with_mode(DEFAULT_REPO, AccessMode::Read).await;
            let _reg = network.register(repo.handle());
            network.add_user_provided_peer(&actor::lookup_addr("alice").await);

            // 2. Sync "b/c.dat"
            common::expect_file_content(&repo, "b/c.dat", &content0).await;
            tx.send(()).await.unwrap();

            // 4. Because the entries are processed in lexicographical order, the blocks of "a.dat" are
            // requested before the blocks of "b". Thus there is a period of time after the snapshot is
            // completed but before the blocks of "b" are downloaded during which the latest version of
            // "b" can't be opened because its blocks haven't been downloaded yet (because the blocks
            // of "a.dat" must be downloaded first). This test verifies that the previous content of
            // "b" can still be accessed even during this period.
            common::eventually(&repo, || async {
                // The first file stays available at all times...
                assert!(
                    common::check_file_version_content(&repo, "b/c.dat", None, &content0).await
                );

                // ...until the new files are transfered
                for (name, content) in [("a.dat", &content1[..]), ("b/d.dat", &[])] {
                    if !common::check_file_version_content(&repo, name, None, content).await {
                        return false;
                    }
                }

                true
            })
            .await;

            tx.send(()).await.unwrap();
        }
    });
}

#[test]
fn redownload_expired_blocks() {
    use std::time::SystemTime;

    let mut env = Env::new();
    let (origin_has_it_tx, mut origin_has_it_rx) = mpsc::channel(1);
    let (cache_had_it_tx, mut cache_had_it_rx) = mpsc::channel(1);
    let (finish_origin_tx, mut finish_origin_rx) = mpsc::channel(1);
    let (finish_cache_tx, mut finish_cache_rx) = mpsc::channel(1);

    let test_content = Arc::new(common::random_bytes(8 * BLOCK_SIZE - BLOB_HEADER_SIZE));

    // Wait until the number of blocks is the `expected`.
    //
    // NOTE: This use sleep instead of waiting for notification events, because no notification
    // events are emitted on block expiration.
    async fn wait_for_block_count(repo: &Repository, expected: u64) {
        let mut backoff = ExponentialBackoffBuilder::new()
            .with_initial_interval(Duration::from_millis(10))
            .with_max_interval(Duration::from_millis(500))
            .with_randomization_factor(0.0)
            .with_multiplier(2.0)
            // .with_max_elapsed_time(Some(Duration::from_secs(60)))
            .with_max_elapsed_time(Some(Duration::from_secs(10)))
            .build();

        loop {
            let actual = repo.count_blocks().await.unwrap();

            if actual == expected {
                return;
            }

            if let Some(duration) = backoff.next_backoff() {
                sleep(duration).await;
            } else {
                panic!("timeout waiting for block count (expected: {expected}, actual: {actual})");
            }
        }
    }

    env.actor("origin", {
        let test_content = test_content.clone();
        async move {
            let (_network, repo, _reg) = actor::setup().await;

            // Create a file
            let mut file = repo.create_file("test.txt").await.unwrap();
            file.write_all(&test_content).await.unwrap();
            file.flush().await.unwrap();

            let block_count = repo.count_blocks().await.unwrap();

            origin_has_it_tx.send(block_count).await.unwrap();

            finish_origin_rx.recv().await;
        }
    });

    env.actor("cache", async move {
        let network = actor::create_network(Proto::Tcp).await;
        let repo = actor::create_repo_with_mode(DEFAULT_REPO, AccessMode::Blind).await;
        let _reg = network.register(repo.handle());

        let block_count = origin_has_it_rx.recv().await.unwrap();

        let start = SystemTime::now();
        network.add_user_provided_peer(&actor::lookup_addr("origin").await);

        // Wait to have the blocks, this is a blind replica, so we can't check the file exists and
        // has the required content.
        wait_for_block_count(&repo, block_count).await;

        let normal_sync_duration = SystemTime::now().duration_since(start).unwrap();

        let expiration_duration = Duration::from_secs(5);

        repo.set_block_expiration(Some(expiration_duration))
            .await
            .unwrap();

        sleep(expiration_duration).await;
        wait_for_block_count(&repo, 0).await;

        cache_had_it_tx
            .send((block_count, normal_sync_duration))
            .await
            .unwrap();

        finish_cache_rx.recv().await;
    });

    env.actor("reader", async move {
        let network = actor::create_network(Proto::Tcp).await;
        let repo = actor::create_repo_with_mode(DEFAULT_REPO, AccessMode::Blind).await;
        let _reg = network.register(repo.handle());

        // Use `start` to measure how long it took to sync the data from the expired cache.
        let (block_count, normal_sync_duration) = cache_had_it_rx.recv().await.unwrap();

        let start = SystemTime::now();
        network.add_user_provided_peer(&actor::lookup_addr("cache").await);

        wait_for_block_count(&repo, block_count).await;

        let expired_sync_duration = SystemTime::now().duration_since(start).unwrap();

        // At time of writing this test, syncing of the expired blocks takes about 2.6 times longer
        // than normal sync. This is perhaps understandable because the 'cache' node needs to
        // download the blocks from 'origin' again and there is also some traffic caused  by the
        // `reader` node requesting blocks from the `cache` node and it responding with "not found"
        // at first.
        //
        // Here we warn if the expired sync is more than 4 times the normal sync (as opposed to the
        // above ratio 2.6), that's because when this test runs with other tests, the actual speed
        // value is less predictable.
        if expired_sync_duration > 4 * normal_sync_duration {
            warn!(
                normal = ?normal_sync_duration,
                expired = ?expired_sync_duration,
                ratio = expired_sync_duration.as_millis() as f64
                    / normal_sync_duration.as_millis() as f64,
                "Sync of expired blocks is more than 4x higher than normal sync"
            );
        }

        finish_origin_tx.send(()).await.unwrap();
        finish_cache_tx.send(()).await.unwrap();
    });
}

#[test]
fn quota_exceed() {
    let mut env = Env::new();

    let quota = StorageSize::from_blocks(4);
    let content0 = common::random_bytes(2 * BLOCK_SIZE - BLOB_HEADER_SIZE);
    let content1 = common::random_bytes(2 * BLOCK_SIZE - BLOB_HEADER_SIZE);
    let content2 = common::random_bytes(BLOCK_SIZE - BLOB_HEADER_SIZE);

    let (tx, mut rx) = mpsc::channel(1);

    env.actor("writer", {
        let content0 = content0.clone();
        let content2 = content2.clone();

        async move {
            let (network, repo, _reg) = actor::setup().await;
            network.add_user_provided_peer(&actor::lookup_addr("reader").await);

            let mut file = repo.create_file("0.dat").await.unwrap();
            common::write_in_chunks(&mut file, &content0, 4096).await;
            file.flush().await.unwrap();
            info!("write 0.dat");

            rx.recv().await.unwrap();

            let mut file = repo.create_file("1.dat").await.unwrap();
            common::write_in_chunks(&mut file, &content1, 4096).await;
            file.flush().await.unwrap();
            drop(file);
            info!("write 1.dat");

            rx.recv().await.unwrap();

            repo.remove_entry("1.dat").await.unwrap();
            info!("remove 1.dat");

            let mut file = repo.create_file("2.dat").await.unwrap();
            common::write_in_chunks(&mut file, &content2, 4096).await;
            file.flush().await.unwrap();
            info!("write 2.dat");

            rx.recv().await.unwrap();
        }
    });

    env.actor("reader", {
        async move {
            let network = actor::create_network(Proto::Tcp).await;

            let params = actor::get_repo_params(DEFAULT_REPO);
            let secrets = actor::get_repo_secrets(DEFAULT_REPO);
            let repo = Repository::create(
                &params,
                Access::new(None, None, secrets.with_mode(AccessMode::Read)),
            )
            .await
            .unwrap();

            repo.set_quota(Some(quota)).await.unwrap();

            let _reg = network.register(repo.handle());

            // The first file is within the quota
            common::expect_file_content(&repo, "0.dat", &content0).await;

            let size0 = repo.size().await.unwrap();
            assert!(
                size0 <= quota,
                "quota exceeded (size: {size0}, quota: {quota})"
            );

            info!("read 0.dat");

            let mut rx = repo.subscribe();

            tx.send(()).await.unwrap();

            // Wait for the next snapshot to be rejected
            loop {
                match common::wait(&mut rx).await {
                    Some(Payload::SnapshotRejected(_)) => break,
                    _ => continue,
                }
            }

            // The second file is rejected because it exceeds the quota
            let size1 = repo.size().await.unwrap();
            assert!(
                size1 <= quota,
                "quota exceeded (size: {size1}, quota: {quota})"
            );

            info!("not read 1.dat");
            tx.send(()).await.unwrap();

            // Once the second file is deleted we accept the third file which is within the quota.
            common::expect_file_content(&repo, "2.dat", &content2).await;

            // The quota might get temporarily exceeded until the old snapshots are pruned.
            common::eventually(&repo, || async {
                let size2 = repo.size().await.unwrap();

                if size2 <= quota {
                    true
                } else {
                    debug!(size = %size2, %quota, "quota exceeded");
                    false
                }
            })
            .await;

            info!("read 2.dat");
            tx.send(()).await.unwrap();
        }
    });
}

#[test]
fn quota_concurrent_writes() {
    let mut env = Env::new();

    let quota = StorageSize::from_blocks(3);

    // Each of these files individually is within the quota but together they exceed it.
    let content0 = common::random_bytes(2 * BLOCK_SIZE - BLOB_HEADER_SIZE);
    let content1 = common::random_bytes(2 * BLOCK_SIZE - BLOB_HEADER_SIZE);

    let barrier = Arc::new(Barrier::new(3));

    for (index, content) in [content0.clone(), content1.clone()].into_iter().enumerate() {
        let barrier = barrier.clone();

        env.actor(&format!("writer-{index}"), async move {
            let (_network, repo, _reg) = actor::setup().await;

            let path = format!("file-{index}.dat");
            let mut file = repo.create_file(&path).await.unwrap();
            common::write_in_chunks(&mut file, &content, 4096).await;
            file.flush().await.unwrap();

            barrier.wait().await;
            barrier.wait().await;
        });
    }

    env.actor("reader", async move {
        let network = actor::create_network(Proto::Tcp).await;
        network.add_user_provided_peer(&actor::lookup_addr("writer-0").await);
        network.add_user_provided_peer(&actor::lookup_addr("writer-1").await);

        let params = actor::get_repo_params(DEFAULT_REPO);
        let secrets = actor::get_repo_secrets(DEFAULT_REPO);
        let repo = Repository::create(
            &params,
            Access::new(None, None, secrets.with_mode(AccessMode::Read)),
        )
        .await
        .unwrap();
        repo.set_quota(Some(quota)).await.unwrap();

        // Wait until all the writes are completed so that we receive only one snapshot from each
        // writer. This simplifies the test.
        barrier.wait().await;

        let mut rx = repo.subscribe();
        let _reg = network.register(repo.handle());

        let mut approved = HashSet::new();
        let mut rejected = HashSet::new();

        loop {
            match common::wait(&mut rx).await {
                Some(Payload::SnapshotApproved(writer_id)) => {
                    approved.insert(writer_id);
                }
                Some(Payload::SnapshotRejected(writer_id)) => {
                    rejected.insert(writer_id);
                }
                Some(_) | None => continue,
            }

            if approved.len() + rejected.len() >= 2 {
                break;
            }
        }

        assert_eq!(approved.len(), 1, "{approved:?}");
        assert_eq!(rejected.len(), 1, "{rejected:?}");

        let size = repo.size().await.unwrap();
        assert!(
            size <= quota,
            "quota exceeded (size: {size}, quota: {quota})"
        );

        barrier.wait().await;
    });
}

#[test]
fn file_progress() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    // make sure some of these are not multiples of BLOCK_SIZE to test edge cases.
    let sizes = [1024 * 1024, 1024 * 1024 + BLOCK_SIZE / 2];

    env.actor("writer", async move {
        let (_network, repo, _reg) = actor::setup().await;

        for size in sizes {
            let mut file = repo.create_file(format!("test-{size}.dat")).await.unwrap();
            let content = common::random_bytes(size);
            common::write_in_chunks(&mut file, &content, 4096).await;
            file.flush().await.unwrap();
        }

        rx.recv().await.unwrap();
    });

    env.actor("reader", async move {
        let (network, repo, _reg) = actor::setup().await;
        network.add_user_provided_peer(&actor::lookup_addr("writer").await);

        common::eventually(&repo, || async {
            let mut complete = 0;

            for size in sizes {
                let path = format!("test-{size}.dat");
                let file = match common::open_file_version(&repo, &path, None).await {
                    Some(file) => file,
                    None => continue,
                };

                let progress = file.progress().await.unwrap();
                let len = file.len();

                debug!(
                    path,
                    "file progress: {}/{} ({:.1}%)",
                    progress,
                    len,
                    100.0 * progress as f64 / len as f64
                );

                assert!(progress <= len);

                if progress == len {
                    complete += 1;
                }
            }

            complete == sizes.len()
        })
        .await;

        tx.send(()).await.unwrap();
    });
}

#[instrument(skip(repo))]
async fn expect_local_directory_exists(repo: &Repository, path: &str) {
    common::eventually(repo, || async {
        debug!(?path, "opening");

        match repo.open_directory(path).await {
            Ok(dir) => dir.has_local_version(),
            Err(
                Error::EntryNotFound
                | Error::Store(StoreError::BlockNotFound)
                // The `LocatorNotFound` error can happen because of fallback: The directory exists
                // in the latest snapshot but some of its blocks are missing. We try to fall back
                // to the previous snapshot but there the directory doesn't exists so it fails with
                // `LocatorNotFound`. This is not a failure in this case because we can wait until
                // the missing blocks are received and try again.
                | Error::Store(StoreError::LocatorNotFound),
            ) => false,
            Err(error) => panic!("unexpected error: {error:?}"),
        }
    })
    .await
}
