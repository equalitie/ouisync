use super::*;

use rand::Rng;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread")]
async fn write_and_read_block() {
    let (_base_dir, store) = setup().await;

    let content = random_block_content();
    let id = BlockId::from_content(&content);
    let nonce = BlockNonce::default();

    let mut tx = store.begin_write().await.unwrap();

    tx.write_block(&id, &content, &nonce).await.unwrap();

    let mut buffer = vec![0; BLOCK_SIZE];
    tx.read_block(&id, &mut buffer).await.unwrap();

    assert_eq!(buffer, content);
}

#[tokio::test(flavor = "multi_thread")]
async fn try_read_missing_block() {
    let (_base_dir, store) = setup().await;

    let mut buffer = vec![0; BLOCK_SIZE];
    let id = BlockId::from_content(&buffer);

    let mut reader = store.acquire_read().await.unwrap();

    match reader.read_block(&id, &mut buffer).await {
        Err(Error::BlockNotFound) => (),
        Err(error) => panic!("unexpected error: {:?}", error),
        Ok(_) => panic!("unexpected success"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn try_write_existing_block() {
    let (_base_dir, store) = setup().await;

    let content0 = random_block_content();
    let id = BlockId::from_content(&content0);
    let nonce = BlockNonce::default();

    let mut tx = store.begin_write().await.unwrap();

    tx.write_block(&id, &content0, &nonce).await.unwrap();
    tx.write_block(&id, &content0, &nonce).await.unwrap();
}

async fn setup() -> (TempDir, Store) {
    let (temp_dir, pool) = db::create_temp().await.unwrap();
    let store = Store::new(pool);
    (temp_dir, store)
}

fn random_block_content() -> Vec<u8> {
    let mut content = vec![0; BLOCK_SIZE];
    rand::thread_rng().fill(&mut content[..]);
    content
}
