use super::*;
use crate::{crypto::SecretKey, error::Error, test_utils};
use assert_matches::assert_matches;
use proptest::collection::vec;
use rand::{distributions::Standard, prelude::*};
use test_strategy::proptest;

#[tokio::test(flavor = "multi_thread")]
async fn empty_blob() {
    let pool = init_db().await;
    let branch = BranchData::new(&pool, rand::random()).await.unwrap();

    let mut blob = Blob::create(pool.clone(), branch.clone(), Cryptor::Null, Locator::Root);
    blob.flush().await.unwrap();

    // Re-open the blob and read its contents.
    let mut blob = Blob::open(pool.clone(), branch, Cryptor::Null, Locator::Root)
        .await
        .unwrap();

    let mut buffer = [0; 1];
    assert_eq!(blob.read(&mut buffer[..]).await.unwrap(), 0);
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
    let (mut rng, cryptor, pool, branch) = setup(rng_seed).await;

    let locator = if is_root {
        Locator::Root
    } else {
        random_head_locator(&mut rng)
    };

    // Create the blob and write to it in chunks of `write_len` bytes.
    let mut blob = Blob::create(pool.clone(), branch.clone(), cryptor.clone(), locator);

    let orig_content: Vec<u8> = rng.sample_iter(Standard).take(blob_len).collect();

    for chunk in orig_content.chunks(write_len) {
        blob.write(chunk).await.unwrap();
    }

    blob.flush().await.unwrap();

    // Re-open the blob and read from it in chunks of `read_len` bytes
    let mut blob = Blob::open(pool.clone(), branch.clone(), cryptor.clone(), locator)
        .await
        .unwrap();

    let mut read_content = vec![0; 0];
    let mut read_buffer = vec![0; read_len];

    loop {
        let len = blob.read(&mut read_buffer[..]).await.unwrap();

        if len == 0 {
            break; // done
        }

        read_content.extend(&read_buffer[..len]);
    }

    assert_eq!(read_content.len(), orig_content.len());
    assert!(read_content == orig_content);
}

#[proptest]
fn len(
    #[strategy(0..3 * BLOCK_SIZE)] content_len: usize,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(async {
        let (rng, cryptor, pool, branch) = setup(rng_seed).await;

        let content: Vec<u8> = rng.sample_iter(Standard).take(content_len).collect();

        let mut blob = Blob::create(pool.clone(), branch.clone(), cryptor.clone(), Locator::Root);
        blob.write(&content[..]).await.unwrap();
        assert_eq!(blob.len(), content_len as u64);

        blob.flush().await.unwrap();
        assert_eq!(blob.len(), content_len as u64);

        let blob = Blob::open(pool.clone(), branch, cryptor.clone(), Locator::Root)
            .await
            .unwrap();
        assert_eq!(blob.len(), content_len as u64);
    })
}

#[proptest]
fn seek_from_start(
    #[strategy(0..2 * BLOCK_SIZE)] content_len: usize,
    #[strategy(0..#content_len)] pos: usize,
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
    #[strategy(0..#content_len)] pos: usize,
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
    let (rng, cryptor, pool, branch) = setup(rng_seed).await;

    let content: Vec<u8> = rng.sample_iter(Standard).take(content_len).collect();

    let mut blob = Blob::create(pool.clone(), branch, cryptor.clone(), Locator::Root);
    blob.write(&content[..]).await.unwrap();
    blob.flush().await.unwrap();

    blob.seek(seek_from).await.unwrap();

    let mut read_buffer = vec![0; content.len()];
    let len = blob.read(&mut read_buffer[..]).await.unwrap();
    assert_eq!(read_buffer[..len], content[expected_pos..]);
}

#[proptest]
fn seek_from_current(
    #[strategy(1..2 * BLOCK_SIZE)] content_len: usize,
    #[strategy(vec(0..#content_len, 1..10))] positions: Vec<usize>,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(async {
        let (rng, cryptor, pool, branch) = setup(rng_seed).await;

        let content: Vec<u8> = rng.sample_iter(Standard).take(content_len).collect();

        let mut blob = Blob::create(pool.clone(), branch, cryptor.clone(), Locator::Root);
        blob.write(&content[..]).await.unwrap();
        blob.flush().await.unwrap();
        blob.seek(SeekFrom::Start(0)).await.unwrap();

        let mut prev_pos = 0;
        for pos in positions {
            blob.seek(SeekFrom::Current(pos as i64 - prev_pos as i64))
                .await
                .unwrap();
            prev_pos = pos;
        }

        let mut read_buffer = vec![0; content.len()];
        let len = blob.read(&mut read_buffer[..]).await.unwrap();
        assert_eq!(read_buffer[..len], content[prev_pos..]);
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn seek_after_end() {
    let (_, cryptor, pool, branch) = setup(0).await;

    let content = b"content";

    let mut blob = Blob::create(pool.clone(), branch, cryptor, Locator::Root);
    blob.write(&content[..]).await.unwrap();
    blob.flush().await.unwrap();

    let mut read_buffer = [0];

    for &offset in &[0, 1, 2] {
        blob.seek(SeekFrom::Start(content.len() as u64 + offset))
            .await
            .unwrap();
        assert_eq!(blob.read(&mut read_buffer).await.unwrap(), 0);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn seek_before_start() {
    let (_, cryptor, pool, branch) = setup(0).await;

    let content = b"content";

    let mut blob = Blob::create(pool.clone(), branch, cryptor, Locator::Root);
    blob.write(&content[..]).await.unwrap();
    blob.flush().await.unwrap();

    let mut read_buffer = vec![0; content.len()];

    for &offset in &[0, 1, 2] {
        blob.seek(SeekFrom::End(-(content.len() as i64) - offset))
            .await
            .unwrap();
        blob.read(&mut read_buffer).await.unwrap();
        assert_eq!(read_buffer, content);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_blob() {
    let (mut rng, cryptor, pool, branch) = setup(0).await;

    let locator0 = random_head_locator(&mut rng);
    let locator1 = locator0.next();

    let content: Vec<_> = (&mut rng)
        .sample_iter(Standard)
        .take(2 * BLOCK_SIZE)
        .collect();

    let mut blob = Blob::create(pool.clone(), branch.clone(), cryptor.clone(), locator0);
    blob.write(&content).await.unwrap();
    blob.flush().await.unwrap();

    let locator0 = locator0.encode(&cryptor);
    let locator1 = locator1.encode(&cryptor);

    let block_ids = {
        let mut tx = pool.begin().await.unwrap();
        let id0 = branch.get(&mut tx, &locator0).await.unwrap();
        let id1 = branch.get(&mut tx, &locator1).await.unwrap();
        [id0, id1]
    };

    // Remove the blob
    blob.remove().await.unwrap();

    // Check the block entries were deleted from the index.
    let mut tx = pool.begin().await.unwrap();
    assert_matches!(
        branch.get(&mut tx, &locator0).await,
        Err(Error::EntryNotFound)
    );
    assert_matches!(
        branch.get(&mut tx, &locator1).await,
        Err(Error::EntryNotFound)
    );

    // Check the blocks were deleted as well.
    for block_id in &block_ids {
        assert!(!block::exists(&mut tx, block_id).await.unwrap());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn truncate_to_empty() {
    let (mut rng, cryptor, pool, branch) = setup(0).await;

    let locator0 = random_head_locator(&mut rng);
    let locator1 = locator0.next();

    let content: Vec<_> = (&mut rng)
        .sample_iter(Standard)
        .take(2 * BLOCK_SIZE)
        .collect();

    let mut blob = Blob::create(pool.clone(), branch.clone(), cryptor.clone(), locator0);
    blob.write(&content).await.unwrap();
    blob.flush().await.unwrap();

    let locator0 = locator0.encode(&cryptor);
    let locator1 = locator1.encode(&cryptor);

    let (old_block_id0, old_block_id1) = {
        let mut tx = pool.begin().await.unwrap();
        let id0 = branch.get(&mut tx, &locator0).await.unwrap();
        let id1 = branch.get(&mut tx, &locator1).await.unwrap();

        (id0, id1)
    };

    blob.truncate(0).await.unwrap();
    blob.flush().await.unwrap();

    // Check the blob is empty
    let mut buffer = [0; 1];
    blob.seek(SeekFrom::Start(0)).await.unwrap();
    assert_eq!(blob.read(&mut buffer).await.unwrap(), 0);
    assert_eq!(blob.len(), 0);

    // Check the second block entry was deleted from the index
    let mut tx = pool.begin().await.unwrap();
    assert_matches!(
        branch.get(&mut tx, &locator1).await,
        Err(Error::EntryNotFound)
    );
    assert!(!block::exists(&mut tx, &old_block_id1).await.unwrap());

    // The first block is not deleted because it's needed to store the metadata.
    // It's only deleted when the blob itself is deleted.
    // Check that it was modified to store the new length though.
    let new_block_id0 = branch.get(&mut tx, &locator0).await.unwrap();
    assert_ne!(new_block_id0, old_block_id0);
    assert!(!block::exists(&mut tx, &old_block_id0).await.unwrap());
    assert!(block::exists(&mut tx, &new_block_id0).await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn truncate_to_shorter() {
    let (mut rng, cryptor, pool, branch) = setup(0).await;

    let locator0 = random_head_locator(&mut rng);
    let locator1 = locator0.next();
    let locator2 = locator1.next();

    let content: Vec<_> = (&mut rng)
        .sample_iter(Standard)
        .take(3 * BLOCK_SIZE)
        .collect();

    let mut blob = Blob::create(pool.clone(), branch.clone(), cryptor.clone(), locator0);
    blob.write(&content).await.unwrap();
    blob.flush().await.unwrap();

    let new_len = BLOCK_SIZE / 2;

    blob.truncate(new_len as u64).await.unwrap();
    blob.flush().await.unwrap();

    let mut buffer = vec![0; new_len];
    blob.seek(SeekFrom::Start(0)).await.unwrap();
    assert_eq!(blob.read(&mut buffer).await.unwrap(), new_len);
    assert_eq!(buffer, content[..new_len]);
    assert_eq!(blob.len(), new_len as u64);

    let mut tx = pool.begin().await.unwrap();
    for locator in &[locator1, locator2] {
        assert_matches!(
            branch.get(&mut tx, &locator.encode(&cryptor)).await,
            Err(Error::EntryNotFound)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn modify_blob() {
    let (mut rng, cryptor, pool, branch) = setup(0).await;

    let locator0 = random_head_locator(&mut rng);
    let locator1 = locator0.next();

    let content = vec![0; 2 * BLOCK_SIZE];
    let mut blob = Blob::create(pool.clone(), branch.clone(), cryptor.clone(), locator0);
    blob.write(&content).await.unwrap();
    blob.flush().await.unwrap();

    let locator0 = locator0.encode(&cryptor);
    let locator1 = locator1.encode(&cryptor);

    let (old_block_id0, old_block_id1) = {
        let mut tx = pool.begin().await.unwrap();
        let id0 = branch.get(&mut tx, &locator0).await.unwrap();
        let id1 = branch.get(&mut tx, &locator1).await.unwrap();
        (id0, id1)
    };

    let buffer = vec![1; 3 * BLOCK_SIZE / 2];
    blob.seek(SeekFrom::Start(0)).await.unwrap();
    blob.write(&buffer).await.unwrap();
    blob.flush().await.unwrap();

    let mut tx = pool.begin().await.unwrap();

    let new_block_id0 = branch.get(&mut tx, &locator0).await.unwrap();
    let new_block_id1 = branch.get(&mut tx, &locator1).await.unwrap();

    assert_ne!(new_block_id0, old_block_id0);
    assert_ne!(new_block_id1, old_block_id1);

    // Check the old blocks were deleted
    for block_id in &[old_block_id0, old_block_id1] {
        assert!(!block::exists(&mut tx, block_id).await.unwrap())
    }
}

#[proptest]
fn fork(
    #[strategy(0..2 * BLOCK_SIZE)] src_len: usize,
    #[strategy(0..=#src_len)] seek_pos: usize,
    #[strategy(1..BLOCK_SIZE)] write_len: usize,
    src_locator_is_root: bool,
    dst_locator_same_as_src: bool,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(fork_case(
        src_len,
        seek_pos,
        write_len,
        src_locator_is_root,
        dst_locator_same_as_src,
        rng_seed,
    ))
}

async fn fork_case(
    src_len: usize,
    seek_pos: usize,
    write_len: usize,
    src_locator_is_root: bool,
    dst_locator_same_as_src: bool,
    rng_seed: u64,
) {
    let (mut rng, cryptor, pool, src_branch) = setup(rng_seed).await;
    let dst_branch = BranchData::new(&pool, rng.gen()).await.unwrap();

    let src_locator = if src_locator_is_root {
        Locator::Root
    } else {
        Locator::Head(rng.gen())
    };

    let dst_locator = if dst_locator_same_as_src {
        src_locator
    } else {
        Locator::Head(rng.gen())
    };

    let src_content: Vec<u8> = (&mut rng).sample_iter(Standard).take(src_len).collect();

    let mut blob = Blob::create(
        pool.clone(),
        src_branch.clone(),
        cryptor.clone(),
        src_locator,
    );
    blob.write(&src_content[..]).await.unwrap();
    blob.flush().await.unwrap();
    blob.seek(SeekFrom::Start(seek_pos as u64)).await.unwrap();

    blob.fork(dst_branch.clone(), dst_locator).await.unwrap();

    let write_content: Vec<u8> = rng.sample_iter(Standard).take(write_len).collect();

    blob.write(&write_content[..]).await.unwrap();
    blob.flush().await.unwrap();

    // Re-open the orig and verify the content is unchanged
    let mut orig = Blob::open(pool.clone(), src_branch, cryptor.clone(), src_locator)
        .await
        .unwrap();

    let buffer = orig.read_to_end().await.unwrap();
    assert_eq!(buffer.len(), src_content.len());
    assert!(buffer == src_content);

    // Re-open the fork and verify the content is changed
    let mut fork = Blob::open(pool.clone(), dst_branch, cryptor, dst_locator)
        .await
        .unwrap();

    let mut buffer = vec![0; seek_pos];
    let len = fork.read(&mut buffer[..]).await.unwrap();
    assert_eq!(len, buffer.len());
    assert!(buffer == src_content[..seek_pos]);

    let mut buffer = vec![0; write_len];
    let len = fork.read(&mut buffer[..]).await.unwrap();
    assert_eq!(len, buffer.len());
    assert!(buffer == write_content);
}

// TODO: test that fork() doesn't create new blocks

async fn setup(rng_seed: u64) -> (StdRng, Cryptor, db::Pool, BranchData) {
    let mut rng = StdRng::seed_from_u64(rng_seed);
    let secret_key = SecretKey::generate(&mut rng);
    let cryptor = Cryptor::ChaCha20Poly1305(secret_key);
    let pool = init_db().await;

    let branch = BranchData::new(&pool, rng.gen()).await.unwrap();

    (rng, cryptor, pool, branch)
}

fn random_head_locator<R: Rng>(rng: &mut R) -> Locator {
    Locator::Head(rng.gen())
}

async fn init_db() -> db::Pool {
    db::init(db::Store::Memory).await.unwrap()
}
