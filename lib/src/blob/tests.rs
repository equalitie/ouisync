use super::*;
use crate::{
    access_control::{AccessKeys, WriteSecrets},
    block::BLOCK_SIZE,
    branch::BranchShared,
    crypto::sign::PublicKey,
    db,
    error::Error,
    event::EventSender,
    index::BranchData,
    store::Store,
    test_utils,
};
use proptest::collection::vec;
use rand::{distributions::Standard, prelude::*};
use tempfile::TempDir;
use test_strategy::proptest;

#[tokio::test(flavor = "multi_thread")]
async fn empty_blob() {
    let (_, _base_dir, store, [branch]) = setup(0).await;
    let mut tx = store.begin_write().await.unwrap();

    let mut blob = Blob::create(branch.clone(), Locator::ROOT);
    blob.flush(&mut tx).await.unwrap();

    // Re-open the blob and read its contents.
    let mut blob = Blob::open(&mut tx, branch, Locator::ROOT).await.unwrap();

    let mut buffer = [0; 1];
    assert_eq!(blob.read(&mut tx, &mut buffer[..]).await.unwrap(), 0);

    drop(tx);
    store.close().await.unwrap();
}

#[proptest]
fn write_and_read(
    is_root: bool,
    #[strategy(1..3 * BLOCK_SIZE)] blob_len: usize,
    #[strategy(1..=#blob_len)] write_len: usize,
    #[strategy(1..=#blob_len + 1)] read_len: usize,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(write_and_read_case(
        is_root, blob_len, write_len, read_len, rng_seed,
    ))
}

async fn write_and_read_case(
    is_root: bool,
    blob_len: usize,
    write_len: usize,
    read_len: usize,
    rng_seed: u64,
) {
    let (mut rng, _base_dir, store, [branch]) = setup(rng_seed).await;

    let mut tx = store.begin_write().await.unwrap();

    let locator = if is_root {
        Locator::ROOT
    } else {
        random_head_locator(&mut rng)
    };

    // Create the blob and write to it in chunks of `write_len` bytes.
    let mut blob = Blob::create(branch.clone(), locator);

    let orig_content: Vec<u8> = rng.sample_iter(Standard).take(blob_len).collect();

    for chunk in orig_content.chunks(write_len) {
        blob.write(&mut tx, chunk).await.unwrap();
    }

    blob.flush(&mut tx).await.unwrap();

    // Re-open the blob and read from it in chunks of `read_len` bytes
    let mut blob = Blob::open(&mut tx, branch.clone(), locator).await.unwrap();

    let mut read_content = vec![0; 0];
    let mut read_buffer = vec![0; read_len];

    loop {
        let len = blob.read(&mut tx, &mut read_buffer[..]).await.unwrap();

        if len == 0 {
            break; // done
        }

        read_content.extend(&read_buffer[..len]);
    }

    assert_eq!(read_content.len(), orig_content.len());
    assert!(read_content == orig_content);

    drop(tx);
    store.close().await.unwrap();
}

#[proptest]
fn len(
    #[strategy(0..3 * BLOCK_SIZE)] content_len: usize,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(async {
        let (rng, _base_dir, store, [branch]) = setup(rng_seed).await;
        let mut tx = store.begin_write().await.unwrap();

        let content: Vec<u8> = rng.sample_iter(Standard).take(content_len).collect();

        let mut blob = Blob::create(branch.clone(), Locator::ROOT);
        blob.write(&mut tx, &content[..]).await.unwrap();
        assert_eq!(blob.len(), content_len as u64);

        blob.flush(&mut tx).await.unwrap();
        assert_eq!(blob.len(), content_len as u64);

        let blob = Blob::open(&mut tx, branch, Locator::ROOT).await.unwrap();
        assert_eq!(blob.len(), content_len as u64);

        drop(tx);
        store.close().await.unwrap();
    })
}

#[proptest]
fn seek_from_start(
    #[strategy(0..2 * BLOCK_SIZE)] content_len: usize,
    #[strategy(0..=#content_len)] pos: usize,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(seek_from(
        content_len,
        SeekFrom::Start(pos as u64),
        pos,
        rng_seed,
    ))
}

#[proptest]
fn seek_from_end(
    #[strategy(0..2 * BLOCK_SIZE)] content_len: usize,
    #[strategy(0..=#content_len)] pos: usize,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(seek_from(
        content_len,
        SeekFrom::End(-((content_len - pos) as i64)),
        pos,
        rng_seed,
    ))
}

async fn seek_from(content_len: usize, seek_from: SeekFrom, expected_pos: usize, rng_seed: u64) {
    let (rng, _base_dir, store, [branch]) = setup(rng_seed).await;
    let mut tx = store.begin_write().await.unwrap();

    let content: Vec<u8> = rng.sample_iter(Standard).take(content_len).collect();

    let mut blob = Blob::create(branch.clone(), Locator::ROOT);
    blob.write(&mut tx, &content[..]).await.unwrap();
    blob.flush(&mut tx).await.unwrap();

    blob.seek(&mut tx, seek_from).await.unwrap();

    let mut read_buffer = vec![0; content.len()];
    let len = blob.read(&mut tx, &mut read_buffer[..]).await.unwrap();
    assert_eq!(read_buffer[..len], content[expected_pos..]);

    drop(tx);
    store.close().await.unwrap();
}

#[proptest]
fn seek_from_current(
    #[strategy(1..2 * BLOCK_SIZE)] content_len: usize,
    #[strategy(vec(0..#content_len, 1..10))] positions: Vec<usize>,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(async {
        let (rng, _base_dir, store, [branch]) = setup(rng_seed).await;
        let mut tx = store.begin_write().await.unwrap();

        let content: Vec<u8> = rng.sample_iter(Standard).take(content_len).collect();

        let mut blob = Blob::create(branch.clone(), Locator::ROOT);
        blob.write(&mut tx, &content[..]).await.unwrap();
        blob.flush(&mut tx).await.unwrap();

        blob.seek(&mut tx, SeekFrom::Start(0)).await.unwrap();

        let mut prev_pos = 0;
        for pos in positions {
            blob.seek(&mut tx, SeekFrom::Current(pos as i64 - prev_pos as i64))
                .await
                .unwrap();
            prev_pos = pos;
        }

        let mut read_buffer = vec![0; content.len()];
        let len = blob.read(&mut tx, &mut read_buffer[..]).await.unwrap();
        assert_eq!(read_buffer[..len], content[prev_pos..]);

        drop(tx);
        store.close().await.unwrap();
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn seek_after_end() {
    let (_, _base_dir, store, [branch]) = setup(0).await;
    let mut tx = store.begin_write().await.unwrap();

    let content = b"content";

    let mut blob = Blob::create(branch.clone(), Locator::ROOT);
    blob.write(&mut tx, &content[..]).await.unwrap();
    blob.flush(&mut tx).await.unwrap();

    let mut read_buffer = [0];

    for &offset in &[0, 1, 2] {
        blob.seek(&mut tx, SeekFrom::Start(content.len() as u64 + offset))
            .await
            .unwrap();

        assert_eq!(blob.read(&mut tx, &mut read_buffer).await.unwrap(), 0);
    }

    drop(tx);
    store.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn seek_before_start() {
    let (_, _base_dir, store, [branch]) = setup(0).await;
    let mut tx = store.begin_write().await.unwrap();

    let content = b"content";

    let mut blob = Blob::create(branch.clone(), Locator::ROOT);
    blob.write(&mut tx, &content[..]).await.unwrap();
    blob.flush(&mut tx).await.unwrap();

    let mut read_buffer = vec![0; content.len()];

    for &offset in &[0, 1, 2] {
        blob.seek(&mut tx, SeekFrom::End(-(content.len() as i64) - offset))
            .await
            .unwrap();
        blob.read(&mut tx, &mut read_buffer).await.unwrap();
        assert_eq!(read_buffer, content);
    }

    drop(tx);
    store.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn truncate_to_empty() {
    let (mut rng, _base_dir, store, [branch]) = setup(0).await;
    let mut tx = store.begin_write().await.unwrap();

    let locator = random_head_locator(&mut rng);

    let content: Vec<_> = (&mut rng)
        .sample_iter(Standard)
        .take(2 * BLOCK_SIZE)
        .collect();

    let mut blob = Blob::create(branch.clone(), locator);
    blob.write(&mut tx, &content).await.unwrap();
    blob.flush(&mut tx).await.unwrap();
    assert_eq!(blob.len(), content.len() as u64);

    blob.truncate(&mut tx, 0).await.unwrap();
    blob.flush(&mut tx).await.unwrap();
    assert_eq!(blob.len(), 0);

    let mut buffer = [0; 1];
    blob.seek(&mut tx, SeekFrom::Start(0)).await.unwrap();
    assert_eq!(blob.read(&mut tx, &mut buffer).await.unwrap(), 0);

    drop(tx);
    store.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn truncate_to_shorter() {
    let (mut rng, _base_dir, store, [branch]) = setup(0).await;
    let mut tx = store.begin_write().await.unwrap();

    let locator = random_head_locator(&mut rng);

    let content: Vec<_> = (&mut rng)
        .sample_iter(Standard)
        .take(3 * BLOCK_SIZE)
        .collect();

    let mut blob = Blob::create(branch.clone(), locator);
    blob.write(&mut tx, &content).await.unwrap();
    blob.flush(&mut tx).await.unwrap();
    assert_eq!(blob.len(), content.len() as u64);

    let new_len = BLOCK_SIZE / 2;

    blob.truncate(&mut tx, new_len as u64).await.unwrap();
    blob.flush(&mut tx).await.unwrap();
    assert_eq!(blob.len(), new_len as u64);

    let mut buffer = vec![0; content.len()];
    blob.seek(&mut tx, SeekFrom::Start(0)).await.unwrap();
    assert_eq!(blob.read(&mut tx, &mut buffer).await.unwrap(), new_len);
    assert_eq!(buffer[..new_len], content[..new_len]);

    drop(tx);
    store.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn truncate_marks_as_dirty() {
    let (mut rng, _base_dir, store, [branch]) = setup(0).await;
    let mut tx = store.begin_write().await.unwrap();

    let locator = random_head_locator(&mut rng);

    let content: Vec<_> = (&mut rng)
        .sample_iter(Standard)
        .take(2 * BLOCK_SIZE)
        .collect();

    let mut blob = Blob::create(branch.clone(), locator);
    blob.write(&mut tx, &content).await.unwrap();
    blob.flush(&mut tx).await.unwrap();

    blob.truncate(&mut tx, 0).await.unwrap();
    assert!(blob.is_dirty());

    drop(tx);
    store.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn modify_blob() {
    let (mut rng, _base_dir, store, [branch]) = setup(0).await;
    let mut tx = store.begin_write().await.unwrap();

    let locator0 = random_head_locator(&mut rng);
    let locator1 = locator0.next();

    let content = vec![0; 2 * BLOCK_SIZE];
    let mut blob = Blob::create(branch.clone(), locator0);
    blob.write(&mut tx, &content).await.unwrap();
    blob.flush(&mut tx).await.unwrap();

    let locator0 = locator0.encode(branch.keys().read());
    let locator1 = locator1.encode(branch.keys().read());

    let (old_block_id0, old_block_id1) = {
        let (id0, _) = tx.find_block(*branch.id(), locator0).await.unwrap();
        let (id1, _) = tx.find_block(*branch.id(), locator1).await.unwrap();
        (id0, id1)
    };

    let buffer = vec![1; 3 * BLOCK_SIZE / 2];
    blob.seek(&mut tx, SeekFrom::Start(0)).await.unwrap();
    blob.write(&mut tx, &buffer).await.unwrap();
    blob.flush(&mut tx).await.unwrap();

    let (new_block_id0, _) = tx.find_block(*branch.id(), locator0).await.unwrap();
    let (new_block_id1, _) = tx.find_block(*branch.id(), locator1).await.unwrap();

    assert_ne!(new_block_id0, old_block_id0);
    assert_ne!(new_block_id1, old_block_id1);

    // Prune snapshots and check the old blocks were deleted
    branch
        .data()
        .load_snapshot(tx.raw_mut())
        .await
        .unwrap()
        .remove_all_older(tx.raw_mut())
        .await
        .unwrap();

    for block_id in &[old_block_id0, old_block_id1] {
        assert!(!tx.block_exists(block_id).await.unwrap())
    }

    drop(tx);
    store.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn append() {
    let (mut rng, _base_dir, store, [branch]) = setup(0).await;
    let mut tx = store.begin_write().await.unwrap();

    let locator = random_head_locator(&mut rng);
    let mut blob = Blob::create(branch.clone(), locator);
    blob.write(&mut tx, b"foo").await.unwrap();
    blob.flush(&mut tx).await.unwrap();

    let mut blob = Blob::open(&mut tx, branch.clone(), locator).await.unwrap();

    blob.seek(&mut tx, SeekFrom::End(0)).await.unwrap();
    blob.write(&mut tx, b"bar").await.unwrap();
    blob.flush(&mut tx).await.unwrap();

    let mut blob = Blob::open(&mut tx, branch, locator).await.unwrap();

    let content = blob.read_to_end(&mut tx).await.unwrap();
    assert_eq!(content, b"foobar");

    drop(tx);
    store.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn write_reopen_and_read() {
    let (mut rng, _base_dir, store, [branch]) = setup(0).await;
    let mut tx = store.begin_write().await.unwrap();

    let locator = random_head_locator(&mut rng);

    let mut blob = Blob::create(branch.clone(), locator);
    blob.write(&mut tx, b"foo").await.unwrap();
    blob.flush(&mut tx).await.unwrap();

    let mut blob = Blob::open(&mut tx, branch, locator).await.unwrap();

    let content = blob.read_to_end(&mut tx).await.unwrap();
    assert_eq!(content, b"foo");

    drop(tx);
    store.close().await.unwrap();
}

#[proptest]
fn fork_and_write(
    #[strategy(0..2 * BLOCK_SIZE)] src_len: usize,
    #[strategy(0..=#src_len)] seek_pos: usize,
    #[strategy(1..BLOCK_SIZE)] write_len: usize,
    src_locator_is_root: bool,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(fork_and_write_case(
        src_len,
        seek_pos,
        write_len,
        src_locator_is_root,
        rng_seed,
    ))
}

async fn fork_and_write_case(
    src_len: usize,
    seek_pos: usize,
    write_len: usize,
    src_locator_is_root: bool,
    rng_seed: u64,
) {
    let (mut rng, _base_dir, store, [src_branch, dst_branch]) = setup(rng_seed).await;

    let src_locator = if src_locator_is_root {
        Locator::ROOT
    } else {
        Locator::head(rng.gen())
    };

    let src_content: Vec<u8> = (&mut rng).sample_iter(Standard).take(src_len).collect();

    let mut tx = store.begin_write().await.unwrap();
    let mut blob = Blob::create(src_branch.clone(), src_locator);
    blob.write(&mut tx, &src_content[..]).await.unwrap();
    blob.flush(&mut tx).await.unwrap();
    tx.commit().await.unwrap();

    fork(*src_locator.blob_id(), &src_branch, &dst_branch)
        .await
        .unwrap();

    let mut tx = store.begin_write().await.unwrap();
    let mut blob = Blob::open(&mut tx, dst_branch.clone(), src_locator)
        .await
        .unwrap();

    let write_content: Vec<u8> = rng.sample_iter(Standard).take(write_len).collect();

    blob.seek(&mut tx, SeekFrom::Start(seek_pos as u64))
        .await
        .unwrap();
    blob.write(&mut tx, &write_content[..]).await.unwrap();
    blob.flush(&mut tx).await.unwrap();

    // Re-open the orig and verify the content is unchanged
    let mut orig = Blob::open(&mut tx, src_branch, src_locator).await.unwrap();

    let buffer = orig.read_to_end(&mut tx).await.unwrap();
    assert_eq!(buffer.len(), src_content.len());
    assert!(buffer == src_content);

    // Re-open the fork and verify the content is changed
    let mut fork = Blob::open(&mut tx, dst_branch, src_locator).await.unwrap();

    let mut buffer = vec![0; seek_pos];
    let len = fork.read(&mut tx, &mut buffer[..]).await.unwrap();
    assert_eq!(len, buffer.len());
    assert!(buffer == src_content[..seek_pos]);

    let mut buffer = vec![0; write_len];
    let len = fork.read(&mut tx, &mut buffer[..]).await.unwrap();
    assert_eq!(len, buffer.len());
    assert!(buffer == write_content);

    drop(tx);
    store.close().await.unwrap();
}

// TODO: test that fork() doesn't create new blocks

#[tokio::test(flavor = "multi_thread")]
async fn fork_is_idempotent() {
    let (mut rng, _base_dir, store, [src_branch, dst_branch]) = setup(0).await;

    let locator = Locator::head(rng.gen());
    let content: Vec<u8> = (&mut rng).sample_iter(Standard).take(512 * 1024).collect();

    let mut tx = store.begin_write().await.unwrap();
    let mut blob = Blob::create(src_branch.clone(), locator);
    blob.write(&mut tx, &content[..]).await.unwrap();
    blob.flush(&mut tx).await.unwrap();
    tx.commit().await.unwrap();

    for i in 0..2 {
        fork(*locator.blob_id(), &src_branch, &dst_branch)
            .await
            .unwrap_or_else(|error| panic!("fork failed in iteration {}: {:?}", i, error));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fork_then_remove_src_branch() {
    let (mut rng, _base_dir, store, [src_branch, dst_branch]) = setup(0).await;

    let locator_0 = Locator::head(rng.gen());
    let locator_1 = Locator::head(rng.gen());

    let mut tx = store.begin_write().await.unwrap();

    let mut blob_0 = Blob::create(src_branch.clone(), locator_0);
    blob_0.flush(&mut tx).await.unwrap();

    let mut blob_1 = Blob::create(src_branch.clone(), locator_1);
    blob_1.flush(&mut tx).await.unwrap();

    tx.commit().await.unwrap();

    fork(*locator_0.blob_id(), &src_branch, &dst_branch)
        .await
        .unwrap();

    drop(blob_0);
    drop(blob_1);

    let mut tx = store.begin_write().await.unwrap();

    // Remove the src branch
    src_branch
        .data()
        .load_snapshot(tx.raw_mut())
        .await
        .unwrap()
        .remove(tx.raw_mut())
        .await
        .unwrap();

    // The forked blob still exists
    Blob::open(&mut tx, dst_branch, locator_0).await.unwrap();

    // The unforked is gone
    match Blob::open(&mut tx, src_branch, locator_1).await {
        Err(Error::Store(store::Error::BranchNotFound)) => (),
        Err(error) => panic!("unexpected error {:?}", error),
        Ok(_) => panic!("unexpected success"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn block_ids_test() {
    let (mut rng, _base_dir, store, [branch]) = setup(0).await;

    let blob_id: BlobId = rng.gen();
    let head_locator = Locator::head(blob_id);
    let mut blob = Blob::create(branch.clone(), head_locator);

    let content: Vec<_> = rng
        .sample_iter(Standard)
        .take(BLOCK_SIZE * 3 - HEADER_SIZE)
        .collect();
    let mut tx = store.begin_write().await.unwrap();
    blob.write(&mut tx, &content).await.unwrap();
    blob.flush(&mut tx).await.unwrap();
    tx.commit().await.unwrap();

    let mut block_ids = BlockIds::open(branch, blob_id).await.unwrap();
    let mut actual_count = 0;

    while block_ids.try_next().await.unwrap().is_some() {
        actual_count += 1;
    }

    assert_eq!(actual_count, 3);

    store.close().await.unwrap();
}

async fn setup<const N: usize>(rng_seed: u64) -> (StdRng, TempDir, Store, [Branch; N]) {
    let mut rng = StdRng::seed_from_u64(rng_seed);
    let keys: AccessKeys = WriteSecrets::generate(&mut rng).into();
    let (base_dir, pool) = db::create_temp().await.unwrap();
    let store = Store::new(pool);

    let event_tx = EventSender::new(1);
    let shared = BranchShared::new();

    let branches = [(); N].map(|_| {
        let id = PublicKey::random();
        let data = BranchData::new(id);
        Branch::new(
            store.clone(),
            data,
            keys.clone(),
            shared.clone(),
            event_tx.clone(),
        )
    });

    (rng, base_dir, store, branches)
}

fn random_head_locator<R: Rng>(rng: &mut R) -> Locator {
    Locator::head(rng.gen())
}
