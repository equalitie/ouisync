use super::{BlockId, BLOCK_SIZE};
use crate::{
    crypto::{
        generic_array::{sequence::GenericSequence, typenum::Unsigned},
        AuthTag,
    },
    db,
    error::{Error, Result},
};
use sqlx::Row;

/// Initializes the block store. Creates the required database schema unless already exists.
pub async fn init(pool: &db::Pool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS blocks (
             id       BLOB NOT NULL PRIMARY KEY,
             auth_tag BLOB NOT NULL,
             content  BLOB NOT NULL
         ) WITHOUT ROWID",
    )
    .execute(pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}

/// Reads a block from the store into a buffer.
///
/// # Panics
///
/// Panics if `buffer` length is less than [`BLOCK_SIZE`].
pub async fn read(tx: &mut db::Transaction, id: &BlockId, buffer: &mut [u8]) -> Result<AuthTag> {
    assert!(
        buffer.len() >= BLOCK_SIZE,
        "insufficient buffer length for block read"
    );

    let row = sqlx::query("SELECT auth_tag, content FROM blocks WHERE id = ?")
        .bind(id.as_array().as_ref())
        .fetch_optional(tx)
        .await?;
    let row = match row {
        Some(row) => row,
        None => return Err(Error::BlockNotFound(*id)),
    };

    let auth_tag: &[u8] = row.get(0);
    if auth_tag.len() != <AuthTag as GenericSequence<_>>::Length::USIZE {
        return Err(Error::MalformedData);
    }
    let auth_tag = AuthTag::clone_from_slice(auth_tag);

    let content: &[u8] = row.get(1);
    if content.len() != BLOCK_SIZE {
        return Err(Error::WrongBlockLength(content.len()));
    }

    buffer.copy_from_slice(content);

    Ok(auth_tag)
}

/// Writes a block into the store.
///
/// # Panics
///
/// Panics if buffer length is not equal to [`BLOCK_SIZE`].
///
pub async fn write(
    tx: &mut db::Transaction,
    id: &BlockId,
    buffer: &[u8],
    auth_tag: &AuthTag,
) -> Result<()> {
    assert_eq!(
        buffer.len(),
        BLOCK_SIZE,
        "incorrect buffer length for block write"
    );

    sqlx::query("INSERT INTO blocks (id, auth_tag, content) VALUES (?, ?, ?)")
        .bind(id.as_array().as_ref())
        .bind(auth_tag.as_slice())
        .bind(buffer)
        .execute(tx)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    #[tokio::test(flavor = "multi_thread")]
    async fn write_and_read() {
        let pool = make_pool().await;
        init(&pool).await.unwrap();

        let id = BlockId::random();
        let content = random_block_content();
        let auth_tag = AuthTag::default();

        let mut tx = pool.begin().await.unwrap();

        write(&mut tx, &id, &content, &auth_tag).await.unwrap();

        let mut buffer = vec![0; BLOCK_SIZE];
        let _ = read(&mut tx, &id, &mut buffer).await.unwrap();

        tx.commit().await.unwrap();

        assert_eq!(buffer, content);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn try_read_missing_block() {
        let pool = make_pool().await;
        init(&pool).await.unwrap();

        let id = BlockId::random();
        let mut buffer = vec![0; BLOCK_SIZE];

        let mut tx = pool.begin().await.unwrap();

        match read(&mut tx, &id, &mut buffer).await {
            Err(Error::BlockNotFound(missing_id)) => assert_eq!(missing_id, id),
            Err(error) => panic!("unexpected error: {:?}", error),
            Ok(_) => panic!("unexpected success"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn try_write_existing_block() {
        let pool = make_pool().await;
        init(&pool).await.unwrap();

        let id = BlockId::random();
        let content0 = random_block_content();
        let auth_tag = AuthTag::default();

        let mut tx = pool.begin().await.unwrap();
        write(&mut tx, &id, &content0, &auth_tag).await.unwrap();
        tx.commit().await.unwrap();

        let content1 = random_block_content();

        let mut tx = pool.begin().await.unwrap();
        match write(&mut tx, &id, &content1, &auth_tag).await {
            Err(Error::QueryDb(_)) => (),
            Err(error) => panic!("unexpected error: {:?}", error),
            Ok(_) => panic!("unexpected success"),
        }
    }

    async fn make_pool() -> db::Pool {
        db::Pool::connect(":memory:").await.unwrap()
    }

    fn random_block_content() -> Vec<u8> {
        let mut content = vec![0; BLOCK_SIZE];
        rand::thread_rng().fill(&mut content[..]);
        content
    }
}
