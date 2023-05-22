//! Synchronization tests

mod common;

use self::common::{actor, Env, Proto, DEFAULT_REPO};
use assert_matches::assert_matches;
use ouisync::{
    Access, AccessMode, EntryType, Error, Repository, StateMonitor, StorageSize, VersionVector,
    BLOB_HEADER_SIZE, BLOCK_SIZE,
};
use rand::Rng;
use std::{cmp::Ordering, io::SeekFrom, sync::Arc};
use tokio::sync::{broadcast, mpsc, Barrier};
use tracing::{instrument, Instrument};

const SMALL_SIZE: usize = 1024;
const LARGE_SIZE: usize = 2 * 1024 * 1024;

#[test]
fn sync_two_peers_one_repo_small() {
    sync_case(2, 1, SMALL_SIZE)
}

#[test]
fn sync_three_peers_one_repo_small() {
    sync_case(3, 1, SMALL_SIZE)
}

#[test]
fn sync_two_peers_one_repo_large() {
    sync_case(2, 1, LARGE_SIZE)
}

#[test]
fn sync_three_peers_one_repo_large() {
    sync_case(3, 1, LARGE_SIZE)
}

#[test]
fn sync_two_peers_two_repos_small() {
    sync_case(2, 2, SMALL_SIZE)
}

#[test]
fn sync_two_peers_two_repos_large() {
    sync_case(2, 2, LARGE_SIZE)
}

fn sync_case(num_peers: usize, num_repos: usize, file_size: usize) {
    assert!(num_peers > 1);
    assert!(num_repos > 0);

    let mut env = Env::new();
    let barrier = Arc::new(Barrier::new(num_peers));

    let contents: Vec<_> = (0..num_repos)
        .map(|_| common::random_content(file_size))
        .collect();

    // Only one file per repo so we can use the same name.
    let file_name = "test.dat";

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
                let mut repos = Vec::with_capacity(num_repos);
                for repo_index in 0..num_repos {
                    repos.push(
                        actor::create_linked_repo(&format!("repo-{repo_index}"), &network).await,
                    );
                }

                for (repo_index, (repo, _)) in repos.iter().enumerate() {
                    // Try to create each file by different actor but if there is more files than
                    // actors then some actors create more than one file.
                    if actor_index != repo_index % num_peers {
                        continue;
                    }

                    async {
                        let mut file = repo.create_file(file_name).await.unwrap();
                        common::write_in_chunks(&mut file, &contents[repo_index], 4096).await;
                        file.flush().await.unwrap();
                    }
                    .instrument(tracing::info_span!(
                        "write",
                        repo = repo_index,
                        name = file_name
                    ))
                    .await
                }

                for (repo_index, content) in contents.iter().enumerate() {
                    common::expect_file_content(&repos[repo_index].0, file_name, content)
                        .instrument(tracing::info_span!(
                            "read",
                            repo = repo_index,
                            name = file_name
                        ))
                        .await;
                }

                barrier.wait().await;
            }
        });
    }
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
        file.write(b"first").await.unwrap();
        file.flush().await.unwrap();

        // Wait for the reader to see the file and then unlink its repo
        reader_rx.recv().await;

        // Modify the file while reader is unlinked
        file.truncate(0).await.unwrap();
        file.write(b"second").await.unwrap();
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
        let _reg = network.register(repo.store().clone()).await;

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

    let content = Arc::new(common::random_content(file_size));

    env.actor("relay", {
        let mut rx = tx.subscribe();

        async move {
            let network = actor::create_network(proto).await;
            let repo = actor::create_repo_with_mode(DEFAULT_REPO, relay_access_mode).await;
            let _reg = network.register(repo.store().clone()).await;

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

#[test]
fn transfer_many_files() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    let num_files = 10;
    let min_size = 0;
    let max_size = 128 * 1024;
    let files = {
        let mut rng = rand::thread_rng();
        let files: Vec<_> = (0..num_files)
            .map(|_| {
                let size = rng.gen_range(min_size..max_size);
                let mut buffer = vec![0; size];
                rng.fill(&mut buffer[..]);
                buffer
            })
            .collect();
        Arc::new(files)
    };

    env.actor("writer", {
        let files = files.clone();

        async move {
            let (_network, repo, _reg) = actor::setup().await;

            for (index, content) in files.iter().enumerate() {
                let name = format!("file-{index}.dat");
                let mut file = repo.create_file(&name).await.unwrap();
                common::write_in_chunks(&mut file, content, 4096).await;
                file.flush().await.unwrap();
            }

            rx.recv().await;
        }
    });

    env.actor("reader", {
        async move {
            let (network, repo, _reg) = actor::setup().await;
            network.add_user_provided_peer(&actor::lookup_addr("writer").await);

            for (index, content) in files.iter().enumerate() {
                let name = format!("file-{index}.dat");
                common::expect_file_content(&repo, &name, content).await;
            }

            tx.send(()).await.unwrap();
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

    let content = common::random_content(3 * BLOCK_SIZE - BLOB_HEADER_SIZE);

    env.actor("alice", {
        let content = content.clone();

        async move {
            let (_network, repo, _reg) = actor::setup().await;

            // Create empty file and wait until bob sees it
            let mut file = repo.create_file("foo.txt").await.unwrap();
            rx.recv().await;

            // Write half of the file content but don't flush yet.
            common::write_in_chunks(&mut file, &content[..content.len() / 2], 4096)
                .instrument(tracing::info_span!("write", name = "foo.txt", step = 1))
                .await;

            // Wait until we see the file created by B
            common::expect_file_content(&repo, "bar.txt", b"bar").await;

            // Write the second half of the content and flush.
            async {
                common::write_in_chunks(&mut file, &content[content.len() / 2..], 4096).await;
                file.flush().await.unwrap();
            }
            .instrument(tracing::info_span!("write", name = "foo.txt", step = 2))
            .await;

            // Reopen the file and verify it has the expected full content
            let mut file = repo.open_file("foo.txt").await.unwrap();
            let actual_content = file
                .read_to_end()
                .instrument(tracing::info_span!("read", name = "foo.txt"))
                .await
                .unwrap();
            common::assert_content_equal(&actual_content, &content);

            rx.recv().await;
        }
    });

    env.actor("bob", {
        async move {
            let (network, repo, _reg) = actor::setup().await;
            network.add_user_provided_peer(&actor::lookup_addr("alice").await);

            let id_bob = *repo.local_branch().unwrap().id();

            // Wait until the file gets merged
            common::expect_file_version_content(&repo, "foo.txt", Some(&id_bob), &[]).await;
            tx.send(()).await.unwrap();

            // Write a file. Excluding the unflushed changes by Alice, this makes Bob's branch newer
            // than Alice's.
            async {
                let mut file = repo.create_file("bar.txt").await.unwrap();
                file.write(b"bar").await.unwrap();
                file.flush().await.unwrap();
            }
            .instrument(tracing::info_span!("write", name = "bar.txt"))
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

    let content_a = Arc::new(common::random_content(2 * BLOCK_SIZE - BLOB_HEADER_SIZE));
    let content_b = Arc::new(common::random_content(2 * BLOCK_SIZE - BLOB_HEADER_SIZE));

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
#[test]
fn recreate_local_branch() {
    let mut env = Env::new();
    let proto = Proto::Tcp;
    let (tx, mut rx) = mpsc::channel(1);

    env.actor("alice", async move {
        let network = actor::create_network(proto).await;

        // 1. Create the repo but don't link it yet.
        let (repo_path, repo_secrets) = actor::get_repo_path_and_secrets(DEFAULT_REPO).await;
        let monitor = StateMonitor::make_root();
        let repo = Repository::create(
            &repo_path,
            actor::device_id(),
            Access::new(None, None, repo_secrets),
            &monitor,
        )
        .await
        .unwrap();

        // 2. Create a file
        let mut file = repo.create_file("foo.txt").await.unwrap();
        file.write(b"hello from Alice\n").await.unwrap();
        file.flush().await.unwrap();
        drop(file);

        let vv_a_0 = repo.local_branch().unwrap().version_vector().await.unwrap();

        // 3. Reopen the repo in read mode to disable merger
        repo.close().await.unwrap();
        drop(repo);

        let repo = Repository::open_with_mode(
            &repo_path,
            actor::device_id(),
            None,
            AccessMode::Read,
            &monitor,
        )
        .await
        .unwrap();

        // 4. Establish link
        let reg = network.register(repo.store().clone()).await;

        // 7. Sync with Bob. Afterwards our local branch will become outdated compared to Bob's
        common::expect_file_content(&repo, "foo.txt", b"hello from Alice\nhello from Bob\n").await;

        // 8: Reopen in write mode
        drop(reg);

        repo.close().await.unwrap();
        drop(repo);
        let repo = Repository::open(&repo_path, actor::device_id(), None, &monitor)
            .await
            .unwrap();

        // 9. Modify the repo
        repo.create_file("bar.txt").await.unwrap();
        repo.force_work().await.unwrap();

        // 10. Make sure the local version changed monotonically.
        let vv_b: VersionVector = rx.recv().await.unwrap();
        assert_eq!(
            vv_b.partial_cmp(&vv_a_0),
            Some(Ordering::Greater),
            "expected {:?} > {:?}",
            vv_b,
            vv_a_0
        );

        let vv_a_1 = repo.local_branch().unwrap().version_vector().await.unwrap();
        assert_eq!(
            vv_a_1.partial_cmp(&vv_b),
            Some(Ordering::Greater),
            "expected {:?} > {:?}",
            vv_a_1,
            vv_b
        );
    });

    env.actor("bob", async move {
        let (network, repo, _reg) = actor::setup().await;
        network.add_user_provided_peer(&actor::lookup_addr("alice").await);

        let id_b = *repo.local_branch().unwrap().id();

        // 5. Sync with Alice
        common::expect_file_version_content(&repo, "foo.txt", Some(&id_b), b"hello from Alice\n")
            .await;
        repo.force_work().await.unwrap();

        // 6. Modify the repo. This makes Bob's branch newer than Alice's
        let mut file = repo.open_file("foo.txt").await.unwrap();
        file.seek(SeekFrom::End(0)).await.unwrap();
        file.write(b"hello from Bob\n").await.unwrap();
        file.flush().await.unwrap();
        drop(file);

        let vv_b = repo.local_branch().unwrap().version_vector().await.unwrap();
        tx.send(vv_b).await.unwrap();
        tx.closed().await;
    });
}

#[test]
fn transfer_directory_with_file() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1); // side-channel

    env.actor("writer", async move {
        let (_network, repo, _reg) = actor::setup().await;

        let mut dir = repo.create_directory("food").await.unwrap();
        dir.create_file("pizza.jpg".into()).await.unwrap();

        rx.recv().await;
    });

    env.actor("reader", async move {
        let (network, repo, _reg) = actor::setup().await;
        network.add_user_provided_peer(&actor::lookup_addr("writer").await);

        common::expect_entry_exists(&repo, "food/pizza.jpg", EntryType::File).await;

        tx.send(()).await.unwrap();
    });
}

#[test]
fn transfer_directory_with_subdirectory() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    env.actor("writer", async move {
        let (_network, repo, _reg) = actor::setup().await;

        let mut dir = repo.create_directory("food").await.unwrap();
        dir.create_directory("mediterranean".into()).await.unwrap();

        rx.recv().await;
    });

    env.actor("reader", async move {
        let (network, repo, _reg) = actor::setup().await;
        network.add_user_provided_peer(&actor::lookup_addr("writer").await);

        common::expect_entry_exists(&repo, "food/mediterranean", EntryType::Directory).await;

        tx.send(()).await.unwrap();
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
        repo.move_entry("/", "foo.txt", "/", "bar.txt")
            .await
            .unwrap();
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
        repo.move_entry("/", "foo", "/", "bar").await.unwrap();
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
        repo.move_entry("/", "foo", "/", "bar").await.unwrap();
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
        repo.create_file("dummy.txt").await.unwrap();

        let _reg = network.register(repo.store().clone()).await;

        repo.create_directory("foo").await.unwrap();
        rx.recv().await;

        repo.move_entry("/", "foo", "/", "bar").await.unwrap();
        rx.recv().await;
    });

    env.actor("reader", async move {
        let network = actor::create_network(proto).await;
        let repo = actor::create_repo(DEFAULT_REPO).await;

        network.add_user_provided_peer(&actor::lookup_addr("writer").await);

        // Create file before linking the repo to ensure we create conflict.
        // This prevents the remote branch from being pruned.
        repo.create_file("dummy.txt").await.unwrap();

        let _reg = network.register(repo.store().clone()).await;

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
        repo.move_entry("/", "data.txt", "archive", "data.txt")
            .await
            .unwrap();
        rx.recv().await;

        repo.move_entry("/", "archive", "/", "trash").await.unwrap();
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
    let content_v0 = common::random_content(2 * BLOCK_SIZE - BLOB_HEADER_SIZE); // exactly 2 blocks

    // Content later edited by overwriting its end with this (shorter than 1 block so that the
    // first block remains unchanged)
    let chunk = common::random_content(64);

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

            let reg = network.register(repo.store().clone()).await;

            // 3. Wait until the file gets merged
            common::expect_file_version_content(&repo, "data.txt", Some(&id_a), &content_v0).await;
            alice_tx.send(()).await.unwrap();

            // 4a. Unlink to allow concurrent operations on the file
            drop(reg);

            // 5a. Remove the file (this garbage-collects its blocks)
            repo.remove_entry("data.txt").await.unwrap();

            // 6a. Relink
            let _reg = network.register(repo.store().clone()).await;

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

            let reg = network.register(repo.store().clone()).await;

            // 2. Create the file and wait until alice sees it
            let mut file = repo.create_file("data.txt").await.unwrap();
            async {
                file.write(&content_v0).await.unwrap();
                file.flush().await.unwrap();
            }
            .instrument(tracing::info_span!("write", name = "data.txt", step = 1))
            .await;

            alice_rx.recv().await.unwrap();

            // 4b. Unlink to allow concurrent operations on the file
            drop(reg);

            // 5b. Writes to the file in such a way that the first block remains unchanged
            async {
                file.seek(SeekFrom::End(-(chunk.len() as i64)))
                    .await
                    .unwrap();
                file.write(&chunk).await.unwrap();
                file.flush().await.unwrap();
            }
            .instrument(tracing::info_span!("write", name = "data.txt", step = 2))
            .await;

            // 6b. Relink
            let _reg = network.register(repo.store().clone()).await;

            alice_rx.recv().await.unwrap();
        }
    });
}

// https://github.com/equalitie/ouisync/issues/86
#[test]
fn content_stays_available_during_sync() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    let content0 = common::random_content(32);
    let content1 = common::random_content(128 * 1024);

    env.actor("alice", {
        let content0 = content0.clone();
        let content1 = content1.clone();

        async move {
            let (_network, repo, _reg) = actor::setup().await;

            // 1. Create "b/c.dat" and wait for Bob to see it.
            let mut file = repo.create_file("b/c.dat").await.unwrap();
            file.write(&content0).await.unwrap();
            file.flush().await.unwrap();

            rx.recv().await.unwrap();

            // 3. Then create "a.dat" and "b/d.dat".
            let mut file = repo.create_file("a.dat").await.unwrap();
            file.write(&content1).await.unwrap();
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
            let _reg = network.register(repo.store().clone()).await;
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

// TODO: unignore when quota is implemented
#[ignore]
#[test]
fn quota() {
    let mut env = Env::new();

    let quota = StorageSize::from_blocks(2);
    let content0 = common::random_content(3 * quota.to_bytes() as usize / 4);
    let content1 = common::random_content(2 * quota.to_bytes() as usize / 4);

    let (tx, mut rx) = mpsc::channel(1);

    env.actor("writer", {
        async move {
            let (network, repo, _reg) = actor::setup().await;
            network.add_user_provided_peer(&actor::lookup_addr("mirror").await);

            let mut file = repo.create_file("one.dat").await.unwrap();
            common::write_in_chunks(&mut file, &content0, 4096).await;
            file.flush().await.unwrap();

            rx.recv().await.unwrap();

            let mut file = repo.create_file("two.dat").await.unwrap();
            common::write_in_chunks(&mut file, &content1, 4096).await;
            file.flush().await.unwrap();

            rx.recv().await.unwrap();
        }
    });

    env.actor("mirror", {
        async move {
            let network = actor::create_network(Proto::Tcp).await;

            let repo = actor::create_repo_with_mode(DEFAULT_REPO, AccessMode::Blind).await;
            repo.set_quota(Some(quota)).await.unwrap();

            let _reg = network.register(repo.store().clone()).await;

            expect_sync_complete(&repo).await;

            // The first file is synced now.
            let size = repo.size().await.unwrap();
            assert!(size < quota);
            tx.send(()).await.unwrap();

            expect_sync_complete(&repo).await;

            // The second file is rejected because it exceeds the quota.
            assert_eq!(repo.size().await.unwrap(), size);
            tx.send(()).await.unwrap();
        }
    });
}

#[instrument(skip(repo))]
async fn expect_local_directory_exists(repo: &Repository, path: &str) {
    common::eventually(repo, || async {
        match repo.open_directory(path).await {
            Ok(dir) => dir.has_local_version(),
            Err(Error::EntryNotFound | Error::BlockNotFound(_)) => false,
            Err(error) => panic!("unexpected error: {error:?}"),
        }
    })
    .await
}

async fn expect_sync_complete(repo: &Repository) {
    common::eventually(repo, || async {
        let progress = repo.sync_progress().await.unwrap();
        progress.total > 0 && progress.value == progress.total
    })
    .await
}
