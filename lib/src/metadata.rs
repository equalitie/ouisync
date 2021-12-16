// TODO: Remove dead code once this is used
#![allow(dead_code)]

use crate::{
    crypto::{AuthTag, Cryptor, Nonce, ScryptSalt, SecretKey},
    db,
    error::{Error, Result},
    repository::RepositoryId,
};
use sqlx::Row;

// Metadata keys
const REPOSITORY_ID: &[u8] = b"repository_id";
const PASSWORD_SALT: &[u8] = b"password_salt";

/// Initialize the metadata tables for storing Key:Value pairs.  One table stores plaintext values,
/// the other one stores encrypted ones.
pub(crate) async fn init(pool: &db::Pool) -> Result<(), Error> {
    // For storing unencrypted values
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS metadata_public (
             name  BLOB NOT NULL PRIMARY KEY,
             value BLOB NOT NULL
         ) WITHOUT ROWID",
    )
    .execute(&*pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    // For storing encrypted values
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS metadata_secret (
             name  BLOB NOT NULL PRIMARY KEY,
             nonce BLOB NOT NULL,
             auth_tag BLOB NOT NULL,
             value BLOB NOT NULL
         ) WITHOUT ROWID",
    )
    .execute(&*pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}

// -------------------------------------------------------------------
// Salt
// -------------------------------------------------------------------
async fn password_salt(db: &db::Pool) -> Result<ScryptSalt> {
    let mut tx = db.begin().await?;

    let salt: Result<ScryptSalt> = get_public(PASSWORD_SALT, &mut tx).await;

    let salt = match salt {
        Ok(salt) => salt,
        Err(Error::EntryNotFound) => {
            let salt = SecretKey::generate_scrypt_salt();
            set_public(PASSWORD_SALT, &salt, &mut tx).await?;
            salt
        }
        Err(e) => return Err(e),
    };

    tx.commit().await?;

    Ok(salt)
}

// -------------------------------------------------------------------
// Master key
// -------------------------------------------------------------------
/// Derive the master key from the user provided password and the salt that is stored in the
/// metadata_plaintext table.
pub(crate) async fn derive_master_key(user_password: &str, db: &db::Pool) -> Result<SecretKey> {
    let salt = password_salt(db).await?;
    Ok(SecretKey::derive_scrypt(user_password, &salt))
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

async fn set_secret(id: &[u8], blob: &[u8], key: &SecretKey, pool: db::Pool) -> Result<()> {
    let cryptor = Cryptor::ChaCha20Poly1305(key.clone());

    let mut tx = pool.begin().await?;

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
    .execute(&mut tx)
    .await?;

    if result.rows_affected() > 0 {
        tx.commit().await?;
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
        init(&pool).await.unwrap();
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
    async fn store_salt() {
        let pool = new_memory_db().await;

        let salt1 = password_salt(&pool).await.unwrap();
        let salt2 = password_salt(&pool).await.unwrap();

        assert_eq!(salt1, salt2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_cyphertext() {
        let pool = new_memory_db().await;

        let key = SecretKey::random();

        set_secret(b"hello", b"world", &key, pool.clone())
            .await
            .unwrap();

        let v: [u8; 5] = get_secret(b"hello", &key, pool.clone()).await.unwrap();

        assert_eq!(b"world", &v);
    }
}
