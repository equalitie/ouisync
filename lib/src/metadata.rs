// TODO: Remove dead code once this is used
#![allow(dead_code)]

use crate::{
    crypto::{sign, AuthTag, Cryptor, Hashable, Nonce, PasswordSalt, SecretKey},
    db,
    error::{Error, Result},
    repository::{MasterSecret, RepositoryId},
};
use rand::{rngs::OsRng, Rng};
use sqlx::Row;

struct AccessSecrets {
    write_key: sign::SecretKey,
    // The read_key is calculated as a hash of the write_key.
    read_key: SecretKey,
    // The public part corresponding to write_key
    repo_id: RepositoryId,
}

impl AccessSecrets {
    fn generate() -> Self {
        let keypair = sign::Keypair::generate();

        AccessSecrets {
            write_key: keypair.secret,
            read_key: keypair.public.as_ref().hash().into(),
            repo_id: keypair.public.into(),
        }
    }
}

// Metadata keys
const REPOSITORY_ID: &[u8] = b"repository_id";
const PASSWORD_SALT: &[u8] = b"password_salt";
const WRITER_ID: &[u8] = b"writer_id";
const WRITER_KEY: &[u8] = b"writer_key";
const READER_KEY: &[u8] = b"reader_key";

/// Initialize the metadata tables for storing Key:Value pairs.  One table stores plaintext values,
/// the other one stores encrypted ones.
///
/// If `master_secret` is `Some(MasterSecret::Password)`, that password shall be converted to
/// SecretKey. If `master_secret` is `Some(MasterSecret::SecretKey)` that key is returned.
pub(crate) async fn init(
    pool: &db::Pool,
    master_secret: Option<MasterSecret>,
) -> Result<Option<SecretKey>, Error> {
    let mut tx = pool.begin().await?;

    if is_initialized(&mut tx).await? {
        let key = match master_secret {
            Some(secret) => Some(secret_to_key(secret, &mut tx).await?),
            None => None,
        };
        return Ok(key);
    }

    let master_secret = match master_secret {
        None => return Err(Error::RepoInitializationRequiresMasterSecret),
        Some(master_secret) => master_secret,
    };

    // For storing unencrypted values
    sqlx::query(
        "CREATE TABLE metadata_public (
             name  BLOB NOT NULL PRIMARY KEY,
             value BLOB NOT NULL,

             UNIQUE(name)
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
             value BLOB NOT NULL,

             UNIQUE(name),
             UNIQUE(nonce)
         ) WITHOUT ROWID",
    )
    .execute(&mut tx)
    .await
    .map_err(Error::CreateDbSchema)?;

    // The salt is only essential for password hashing. When the repo is initialized with
    // MasterSecret::Secretkey it's not required. But we generate it anyway so as to not leak the
    // information which (SecretKey or Password) was the repo initialized with.
    generate_password_salt(&mut tx).await?;
    let master_key = secret_to_key(master_secret, &mut tx).await?;

    let secrets = AccessSecrets::generate();

    // TODO: At the moment, writer keys are just random bytes. This is because it is a long term
    // plan (which may or may not materialize) to have a writer set CRDT structure indicating who
    // can write to the repository. This structure would collect sign::PublicKeys and as such,
    // users would locally store their private signing keys corresponding to those in the writer
    // set instead of storing a "global" sign::PublicKey.
    set_writer_id(&OsRng.gen(), &master_key, &mut tx).await?;

    set_secret(
        REPOSITORY_ID,
        secrets.repo_id.as_ref(),
        &master_key,
        &mut tx,
    )
    .await?;

    set_secret(WRITER_KEY, secrets.write_key.as_ref(), &master_key, &mut tx).await?;

    set_secret(
        READER_KEY,
        secrets.read_key.as_array(),
        &master_key,
        &mut tx,
    )
    .await?;

    tx.commit().await?;

    Ok(Some(master_key))
}

async fn secret_to_key(secret: MasterSecret, db: impl db::Executor<'_>) -> Result<SecretKey> {
    let key = match secret {
        MasterSecret::SecretKey(key) => key,
        MasterSecret::Password(pwd) => {
            let salt = get_password_salt(db)
                .await?
                .expect("database is not initialized");
            SecretKey::derive_from_password(pwd.as_ref(), &salt)
        }
    };

    Ok(key)
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
async fn get_password_salt(db: impl db::Executor<'_>) -> Result<Option<PasswordSalt>> {
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
// Writer Id
// -------------------------------------------------------------------
pub(crate) async fn get_writer_id(
    key: &Option<SecretKey>,
    db: impl db::Executor<'_>,
) -> Result<sign::PublicKey> {
    let id = match key {
        Some(key) => get_secret(WRITER_ID, key, db)
            .await?
            .map(|blob| blob.into())
            .into(),
        None => OsRng.gen(),
    };
    Ok(id)
}

pub(crate) async fn set_writer_id(
    writer_id: &sign::PublicKey,
    key: &SecretKey,
    db: impl db::Executor<'_>,
) -> Result<()> {
    set_secret(WRITER_ID, writer_id.as_ref(), key, db).await
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
async fn get_secret<const N: usize>(
    id: &[u8],
    key: &SecretKey,
    db: impl db::Executor<'_>,
) -> Result<[u8; N]> {
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
            .fetch_optional(db)
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

        let v: [u8; 5] = get_secret(b"hello", &key, &pool).await.unwrap();

        assert_eq!(b"world", &v);
    }
}
