use super::{BlockId, BLOCK_SIZE};
use crate::{
    crypto::cipher::AuthTag,
    db,
    error::{Error, Result},
};
use generic_array::{sequence::GenericSequence, typenum::Unsigned};
use sqlx::{sqlite::SqliteRow, Row};

/// Initializes the block store. Creates the required database schema unless already exists.
pub async fn init(conn: &mut db::Connection) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS blocks (
             id       BLOB NOT NULL PRIMARY KEY,
             auth_tag BLOB NOT NULL,
             content  BLOB NOT NULL
         ) WITHOUT ROWID",
    )
    .execute(conn)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}

/// Reads a block from the store into a buffer.
///
/// # Panics
///
/// Panics if `buffer` length is less than [`BLOCK_SIZE`].
pub(crate) async fn read(
    conn: &mut db::Connection,
    id: &BlockId,
    buffer: &mut [u8],
) -> Result<AuthTag> {
    let row = sqlx::query("SELECT auth_tag, content FROM blocks WHERE id = ?")
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| Error::BlockNotFound(*id))?;

    from_row(row, buffer)
}

fn from_row(row: SqliteRow, buffer: &mut [u8]) -> Result<AuthTag> {
    assert!(
        buffer.len() >= BLOCK_SIZE,
        "insufficient buffer length for block read"
    );

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
/// This function is idempotent, that is, writing the same block (same id, same content) multiple
/// times has the same effect as writing it only once. However, attempt to write a block with the
/// same id as an existing block but with a different content or auth_tag is an error.
///
/// # Panics
///
/// Panics if buffer length is not equal to [`BLOCK_SIZE`].
///
pub(crate) async fn write(
    conn: &mut db::Connection,
    id: &BlockId,
    buffer: &[u8],
    auth_tag: &AuthTag,
) -> Result<()> {
    assert_eq!(
        buffer.len(),
        BLOCK_SIZE,
        "incorrect buffer length for block write"
    );

    let result = sqlx::query(
        "INSERT INTO blocks (id, auth_tag, content)
         VALUES (?, ?, ?)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(id)
    .bind(auth_tag.as_slice())
    .bind(buffer)
    .execute(&mut *conn)
    .await?;

    if result.rows_affected() > 0 {
        Ok(())
    } else {
        // Block with the same id already exists. If it also has the same content, treat it as a
        // success (to make this function idempotent), otherwise error.
        if sqlx::query("SELECT content = ? AND auth_tag = ? FROM blocks WHERE id = ?")
            .bind(buffer)
            .bind(auth_tag.as_slice())
            .bind(id)
            .fetch_one(conn)
            .await?
            .get(0)
        {
            Ok(())
        } else {
            Err(Error::BlockExists)
        }
    }
}

/// Checks whether a block exists in the store.
/// (Currently used only in tests)
#[cfg(test)]
pub(crate) async fn exists(conn: &mut db::Connection, id: &BlockId) -> Result<bool> {
    Ok(sqlx::query("SELECT 0 FROM blocks WHERE id = ?")
        .bind(id)
        .fetch_optional(conn)
        .await?
        .is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    #[tokio::test(flavor = "multi_thread")]
    async fn write_and_read() {
        let mut conn = setup().await;

        let id = rand::random();
        let content = random_block_content();
        let auth_tag = AuthTag::default();

        write(&mut conn, &id, &content, &auth_tag).await.unwrap();

        let mut buffer = vec![0; BLOCK_SIZE];
        read(&mut conn, &id, &mut buffer).await.unwrap();

        assert_eq!(buffer, content);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn try_read_missing_block() {
        let mut conn = setup().await;

        let id = rand::random();
        let mut buffer = vec![0; BLOCK_SIZE];

        match read(&mut conn, &id, &mut buffer).await {
            Err(Error::BlockNotFound(missing_id)) => assert_eq!(missing_id, id),
            Err(error) => panic!("unexpected error: {:?}", error),
            Ok(_) => panic!("unexpected success"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn try_write_existing_block() {
        let mut conn = setup().await;

        let id = rand::random();
        let content0 = random_block_content();
        let auth_tag = AuthTag::default();

        write(&mut conn, &id, &content0, &auth_tag).await.unwrap();

        // Try to overwrite it with the same content -> no-op.
        write(&mut conn, &id, &content0, &auth_tag).await.unwrap();

        // Try to overwrite it with a different content -> error
        let content1 = random_block_content();
        match write(&mut conn, &id, &content1, &auth_tag).await {
            Err(Error::BlockExists) => (),
            Err(error) => panic!("unexpected error: {:?}", error),
            Ok(_) => panic!("unexpected success"),
        }
    }

    async fn setup() -> db::Connection {
        let mut conn = db::open_or_create(&db::Store::Memory)
            .await
            .unwrap()
            .acquire()
            .await
            .unwrap()
            .detach();
        init(&mut conn).await.unwrap();
        conn
    }

    fn random_block_content() -> Vec<u8> {
        let mut content = vec![0; BLOCK_SIZE];
        rand::thread_rng().fill(&mut content[..]);
        content
    }
}
