use crate::{
    db,
    error::{Error, Result},
    master_key::{MasterKey, Salt},
    repository::SecretRepositoryId,
};
use sqlx::Row;

// Metadata keys
const REPOSITORY_ID: &[u8] = b"repository_id";
const PASSWORD_SALT: &[u8] = b"password_salt";

pub(crate) async fn init(pool: &db::Pool) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS metadata_plaintext (
             name  BLOB NOT NULL PRIMARY KEY,
             value BLOB NOT NULL
         ) WITHOUT ROWID",
    )
    .execute(&*pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}

async fn password_salt(db: db::Pool) -> Result<Salt> {
    let mut tx = db.begin().await?;

    let salt: Result<Salt> = get_plaintext(PASSWORD_SALT, &mut tx).await;

    let salt = match salt {
        Ok(salt) => salt,
        Err(Error::EntryNotFound) => {
            let salt = MasterKey::generate_salt();
            set_plaintext(PASSWORD_SALT, &salt, &mut tx).await?;
            salt
        }
        Err(e) => return Err(e),
    };

    tx.commit().await?;

    Ok(salt)
}

// -------------------------------------------------------------------
// Repository Id
// -------------------------------------------------------------------
pub(crate) async fn get_repository_id(db: impl db::Executor<'_>) -> Result<SecretRepositoryId> {
    get_plaintext(REPOSITORY_ID, db)
        .await
        .map(|blob| blob.into())
}

pub(crate) async fn set_repository_id(
    db: impl db::Executor<'_>,
    id: &SecretRepositoryId,
) -> Result<()> {
    set_plaintext(REPOSITORY_ID, id.as_ref(), db).await
}

// -------------------------------------------------------------------

async fn get_plaintext<const N: usize>(id: &[u8], db: impl db::Executor<'_>) -> Result<[u8; N]> {
    sqlx::query("SELECT value FROM metadata_plaintext WHERE name = ?")
        .bind(id)
        .map(|row| (row.get::<'_, &[u8], usize>(0)).try_into().unwrap())
        .fetch_optional(db)
        .await?
        .ok_or(Error::EntryNotFound)
}

async fn set_plaintext(id: &[u8], blob: &[u8], db: impl db::Executor<'_>) -> Result<()> {
    let result = sqlx::query(
        "INSERT INTO metadata_plaintext(name, value) VALUES (?, ?) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(blob)
    .execute(db)
    .await?;

    if result.rows_affected() > 0 {
        Ok(())
    } else {
        Err(Error::EntryExists)
    }
}
