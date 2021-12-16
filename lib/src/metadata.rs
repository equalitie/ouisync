// TODO: Remove dead code once this is used
#![allow(dead_code)]

use crate::{
    crypto::{AuthTag, Cryptor, Nonce, PasswordSalt, SecretKey},
    db,
    error::{Error, Result},
    repository::{RepositoryId, MasterSecret},
};
use sqlx::Row;

// Metadata keys
const REPOSITORY_ID: &[u8] = b"repository_id";
const PASSWORD_SALT: &[u8] = b"password_salt";
const READER_KEY: &[u8] = b"reader_key";

/// Initialize the metadata tables for storing Key:Value pairs.  One table stores plaintext values,
/// the other one stores encrypted ones.
pub(crate) async fn init(
    pool: &db::Pool,
    master_secret: Option<MasterSecret>,
) -> Result<(), Error> {
    let mut tx = pool.begin().await?;

    if is_initialized(&mut tx).await? {
        return Ok(());
    }

    if master_secret.is_none() {
        return Err(Error::RepoInitializationRequiresMasterSecret);
    }

    // For storing unencrypted values
    sqlx::query(
        "CREATE TABLE metadata_public (
             name  BLOB NOT NULL PRIMARY KEY,
             value BLOB NOT NULL
         ) WITHOUT ROWID",
    )
    .execute(&mut tx)
    .await
    .map_err(Error::CreateDbSchema)?;

    // For storing encrypted values
    sqlx::query(
        "CREATE TABLE metadata_secret (
             name  BLOB NOT NULL PRIMARY KEY,
             nonce BLOB NOT NULL,
             auth_tag BLOB NOT NULL,
             value BLOB NOT NULL
         ) WITHOUT ROWID",
    )
    .execute(&mut tx)
    .await
    .map_err(Error::CreateDbSchema)?;

    // The salt is only really required for password hashing. When the repo is initialized with
    // MasterSecret::Secretkey it's not required. But we generate it anyway as to not leak the
    // information which (SecretKey or Password) was the repo initialized with.
    let salt = generate_password_salt(&mut tx).await?;

    tx.commit().await?;

    Ok(())
}

async fn is_initialized(db: impl db::Executor<'_>) -> Result<bool> {
    Ok(
        sqlx::query("SELECT name FROM sqlite_master WHERE type='table' AND name='metadata_public'")
            .fetch_optional(db)
            .await?
            .is_some(),
    )
}

// -------------------------------------------------------------------
// Salt
// -------------------------------------------------------------------
async fn get_password_salt(db: &mut db::Transaction<'_>) -> Result<Option<PasswordSalt>> {
    let salt: Result<PasswordSalt> = get_public(PASSWORD_SALT, db).await;

    match salt {
        Ok(salt) => Ok(Some(salt)),
        Err(Error::EntryNotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

async fn generate_password_salt(db: &mut db::Transaction<'_>) -> Result<PasswordSalt> {
    let salt = SecretKey::generate_password_salt();
    set_public(PASSWORD_SALT, &salt, db).await?;
    Ok(salt)
}

// -------------------------------------------------------------------
// Repository Id
// -------------------------------------------------------------------
pub(crate) async fn get_repository_id(db: impl db::Executor<'_>) -> Result<RepositoryId> {
    get_public(REPOSITORY_ID, db).await.map(|blob| blob.into())
}

pub(crate) async fn set_repository_id(db: impl db::Executor<'_>, id: &RepositoryId) -> Result<()> {
    set_public(REPOSITORY_ID, id.as_ref(), db).await
}

// -------------------------------------------------------------------
// Public values
// -------------------------------------------------------------------
async fn get_public<const N: usize>(id: &[u8], db: impl db::Executor<'_>) -> Result<[u8; N]> {
    sqlx::query("SELECT value FROM metadata_public WHERE name = ?")
        .bind(id)
        .map(|row| (row.get::<'_, &[u8], usize>(0)).try_into().unwrap())
        .fetch_optional(db)
        .await?
        .ok_or(Error::EntryNotFound)
}

async fn set_public(id: &[u8], blob: &[u8], db: impl db::Executor<'_>) -> Result<()> {
    let result = sqlx::query(
        "INSERT INTO metadata_public(name, value) VALUES (?, ?) ON CONFLICT DO NOTHING",
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

// -------------------------------------------------------------------
// Secret values
// -------------------------------------------------------------------
async fn get_secret<const N: usize>(id: &[u8], key: &SecretKey, db: db::Pool) -> Result<[u8; N]> {
    let (nonce, auth_tag, mut value): (Nonce, AuthTag, [u8; N]) =
        sqlx::query("SELECT nonce, auth_tag, value FROM metadata_secret WHERE name = ?")
            .bind(id)
            .map(|row| {
                let nonce: &[u8] = row.get(0);
                let auth_tag: &[u8] = row.get(1);
                let value: &[u8] = row.get(2);
                (
                    nonce.try_into().unwrap(),
                    AuthTag::clone_from_slice(auth_tag),
                    value.try_into().unwrap(),
                )
            })
            .fetch_optional(&db)
            .await?
            .ok_or(Error::EntryNotFound)?;

    let cryptor = Cryptor::ChaCha20Poly1305(key.clone());

    cryptor.decrypt(nonce, id, &mut value, &auth_tag)?;

    Ok(value)
}

async fn set_secret(
    id: &[u8],
    blob: &[u8],
    key: &SecretKey,
    db: impl db::Executor<'_>,
) -> Result<()> {
    let cryptor = Cryptor::ChaCha20Poly1305(key.clone());

    let nonce = make_nonce();

    let mut cypher = blob.to_vec();
    let auth_tag = cryptor.encrypt(nonce, id, cypher.as_mut_slice())?;

    let result = sqlx::query(
        "INSERT INTO metadata_secret(name, nonce, auth_tag, value)
            VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(&nonce[..])
    .bind(&*auth_tag)
    .bind(&cypher)
    .execute(db)
    .await?;

    if result.rows_affected() > 0 {
        Ok(())
    } else {
        Err(Error::EntryExists)
    }
}

fn make_nonce() -> Nonce {
    // Random nonces should be OK given that we're not generating too many of them.
    // But maybe consider using the mixed approach from this SO post?
    // https://crypto.stackexchange.com/a/77986
    rand::random()
}

// -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn new_memory_db() -> db::Pool {
        let pool = db::open(&db::Store::Memory).await.unwrap();
        let master_secret = Some(MasterSecret::SecretKey(SecretKey::random()));
        init(&pool, master_secret).await.unwrap();
        pool
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_plaintext() {
        let pool = new_memory_db().await;

        let repo_id = rand::random();
        set_repository_id(&pool, &repo_id).await.unwrap();

        let repo_id_ = get_repository_id(&pool).await.unwrap();

        assert_eq!(repo_id, repo_id_);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_cyphertext() {
        let pool = new_memory_db().await;

        let key = SecretKey::random();

        set_secret(b"hello", b"world", &key, &pool).await.unwrap();

        let v: [u8; 5] = get_secret(b"hello", &key, pool.clone()).await.unwrap();

        assert_eq!(b"world", &v);
    }
}
