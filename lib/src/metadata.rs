use crate::{
    access_control::{AccessSecrets, MasterSecret, WriteSecrets},
    crypto::{
        cipher::{AuthTag, Nonce, SecretKey, AUTH_TAG_SIZE},
        sign, Cryptor, PasswordSalt,
    },
    db,
    error::{Error, Result},
    repository::RepositoryId,
};
use rand::{rngs::OsRng, Rng};
use sqlx::Row;
use zeroize::Zeroize;

// Metadata keys
const REPOSITORY_ID: &[u8] = b"repository_id";
const PASSWORD_SALT: &[u8] = b"password_salt";
const WRITER_ID: &[u8] = b"writer_id";
const ACCESS_KEY: &[u8] = b"access_key"; // read key or write key

/// Initialize the metadata tables for storing Key:Value pairs.  One table stores plaintext values,
/// the other one stores encrypted ones.
pub(crate) async fn init(pool: &db::Pool) -> Result<(), Error> {
    let mut tx = pool.begin().await?;

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
             name     BLOB NOT NULL PRIMARY KEY,
             nonce    BLOB NOT NULL,
             auth_tag BLOB NOT NULL,
             value    BLOB NOT NULL,

             UNIQUE(nonce)
         ) WITHOUT ROWID",
    )
    .execute(&mut tx)
    .await
    .map_err(Error::CreateDbSchema)?;

    tx.commit().await?;

    Ok(())
}

pub(crate) async fn secret_to_key(
    secret: MasterSecret,
    db: impl db::Acquire<'_>,
) -> Result<SecretKey> {
    let key = match secret {
        MasterSecret::SecretKey(key) => key,
        MasterSecret::Password(pwd) => {
            let salt = get_or_generate_password_salt(db).await?;
            SecretKey::derive_from_password(pwd.as_ref(), &salt)
        }
    };

    Ok(key)
}

// -------------------------------------------------------------------
// Salt
// -------------------------------------------------------------------
async fn get_or_generate_password_salt(db: impl db::Acquire<'_>) -> Result<PasswordSalt> {
    let mut tx = db.begin().await?;

    let salt = match get_public(PASSWORD_SALT, &mut tx).await {
        Ok(salt) => salt,
        Err(Error::EntryNotFound) => {
            let salt: PasswordSalt = OsRng.gen();
            insert_public(PASSWORD_SALT, &salt, &mut tx).await?;
            salt
        }
        Err(error) => return Err(error),
    };

    tx.commit().await?;

    Ok(salt)
}

// -------------------------------------------------------------------
// Repository Id
// -------------------------------------------------------------------
pub(crate) async fn get_repository_id(db: impl db::Executor<'_>) -> Result<RepositoryId> {
    get_public(REPOSITORY_ID, db).await
}

pub(crate) async fn set_repository_id(id: &RepositoryId, db: impl db::Executor<'_>) -> Result<()> {
    insert_public(REPOSITORY_ID, id.as_ref(), db).await
}

// -------------------------------------------------------------------
// Writer Id
// -------------------------------------------------------------------
pub(crate) async fn get_writer_id(
    master_key: &Option<SecretKey>,
    db: impl db::Executor<'_>,
) -> Result<sign::PublicKey> {
    let id = match master_key {
        Some(key) => get_secret(WRITER_ID, key, db).await?,
        None => OsRng.gen(),
    };
    Ok(id)
}

pub(crate) async fn set_writer_id(
    writer_id: &sign::PublicKey,
    master_key: &SecretKey,
    db: impl db::Executor<'_>,
) -> Result<()> {
    insert_secret(WRITER_ID, writer_id.as_ref(), master_key, db).await
}

// -------------------------------------------------------------------
// Access secrets
// -------------------------------------------------------------------
pub(crate) async fn set_access_secrets(
    secrets: &AccessSecrets,
    master_key: &SecretKey,
    db: impl db::Acquire<'_>,
) -> Result<()> {
    let mut tx = db.begin().await?;

    replace_public(REPOSITORY_ID, secrets.id().as_ref(), &mut tx).await?;

    match secrets {
        AccessSecrets::Blind { .. } => {
            // Insert a dummy key for plausible deniability.
            let dummy_key: sign::SecretKey = OsRng.gen();
            replace_secret(ACCESS_KEY, dummy_key.as_ref(), master_key, &mut tx).await?;
        }
        AccessSecrets::Read { read_key, .. } => {
            replace_secret(ACCESS_KEY, read_key.as_ref(), master_key, &mut tx).await?;
        }
        AccessSecrets::Write(secrets) => {
            replace_secret(ACCESS_KEY, secrets.write_key.as_ref(), master_key, &mut tx).await?;
        }
    }

    tx.commit().await?;

    Ok(())
}

pub(crate) async fn get_access_secrets(
    master_key: &SecretKey,
    db: impl db::Acquire<'_>,
) -> Result<AccessSecrets> {
    let mut cx = db.acquire().await?;

    let id = get_public(REPOSITORY_ID, &mut *cx).await?;

    // try to interpret it first as the write key
    let write_key: sign::SecretKey = get_secret(ACCESS_KEY, master_key, &mut *cx).await?;
    let derived_id = sign::PublicKey::from(&write_key);
    let derived_id = RepositoryId::from(derived_id);

    if derived_id == id {
        // It was the write key. We have write access.
        Ok(AccessSecrets::Write(WriteSecrets::from(write_key)))
    } else {
        // It wasn't the write key. Either it's the read key or a dummy key. We can't tell so
        // assume it's the read key for now.
        // unwrap is OK because the two secret key types have the same size.
        let read_key = SecretKey::try_from(write_key.as_ref()).unwrap();
        Ok(AccessSecrets::Read { id, read_key })
    }
}

// -------------------------------------------------------------------
// Public values
// -------------------------------------------------------------------
async fn get_public<T>(id: &[u8], db: impl db::Executor<'_>) -> Result<T>
where
    T: for<'a> TryFrom<&'a [u8]>,
{
    let row = sqlx::query("SELECT value FROM metadata_public WHERE name = ?")
        .bind(id)
        .fetch_optional(db)
        .await?;
    let row = row.ok_or(Error::EntryNotFound)?;
    let bytes: &[u8] = row.get(0);
    bytes.try_into().map_err(|_| Error::MalformedData)
}

async fn insert_public(id: &[u8], blob: &[u8], db: impl db::Executor<'_>) -> Result<()> {
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

async fn replace_public(id: &[u8], blob: &[u8], db: impl db::Executor<'_>) -> Result<()> {
    sqlx::query("INSERT OR REPLACE INTO metadata_public(name, value) VALUES (?, ?)")
        .bind(id)
        .bind(blob)
        .execute(db)
        .await?;

    Ok(())
}

// -------------------------------------------------------------------
// Secret values
// -------------------------------------------------------------------
async fn get_secret<T>(id: &[u8], master_key: &SecretKey, db: impl db::Executor<'_>) -> Result<T>
where
    for<'a> T: TryFrom<&'a [u8]>,
{
    let row = sqlx::query("SELECT nonce, auth_tag, value FROM metadata_secret WHERE name = ?")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(Error::EntryNotFound)?;

    let nonce: &[u8] = row.get(0);
    let nonce = Nonce::try_from(nonce)?;

    let auth_tag: &[u8] = row.get(1);
    let auth_tag: [u8; AUTH_TAG_SIZE] = auth_tag.try_into()?;
    let auth_tag = AuthTag::from(auth_tag);

    let mut buffer: Vec<_> = row.get(2);

    let cryptor = Cryptor::ChaCha20Poly1305(master_key.clone());
    cryptor.decrypt(nonce, id, &mut buffer, &auth_tag)?;

    let secret = T::try_from(&buffer).map_err(|_| Error::MalformedData)?;
    buffer.zeroize();

    Ok(secret)
}

async fn insert_secret(
    id: &[u8],
    blob: &[u8],
    master_key: &SecretKey,
    db: impl db::Executor<'_>,
) -> Result<()> {
    let (nonce, cypher, auth_tag) = prepare_secret(id, blob, master_key)?;

    let result = sqlx::query(
        "INSERT INTO metadata_secret(name, nonce, auth_tag, value)
            VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(&nonce[..])
    .bind(&auth_tag[..])
    .bind(&cypher)
    .execute(db)
    .await?;

    if result.rows_affected() > 0 {
        Ok(())
    } else {
        Err(Error::EntryExists)
    }
}

async fn replace_secret(
    id: &[u8],
    blob: &[u8],
    master_key: &SecretKey,
    db: impl db::Executor<'_>,
) -> Result<()> {
    let (nonce, cypher, auth_tag) = prepare_secret(id, blob, master_key)?;

    sqlx::query(
        "INSERT OR REPLACE INTO metadata_secret(name, nonce, auth_tag, value)
            VALUES (?, ?, ?, ?)",
    )
    .bind(id)
    .bind(&nonce[..])
    .bind(&auth_tag[..])
    .bind(&cypher)
    .execute(db)
    .await?;

    Ok(())
}

fn prepare_secret(
    id: &[u8],
    blob: &[u8],
    master_key: &SecretKey,
) -> Result<(Nonce, Vec<u8>, AuthTag)> {
    let cryptor = Cryptor::ChaCha20Poly1305(master_key.clone());
    let nonce = make_nonce();

    let mut cypher = blob.to_vec();
    let auth_tag = cryptor.encrypt(nonce, id, &mut cypher)?;

    Ok((nonce, cypher, auth_tag))
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
        let pool = db::open_or_create(&db::Store::Memory).await.unwrap();
        init(&pool).await.unwrap();
        pool
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_plaintext() {
        let pool = new_memory_db().await;

        let repo_id = rand::random();
        set_repository_id(&repo_id, &pool).await.unwrap();

        let repo_id_ = get_repository_id(&pool).await.unwrap();

        assert_eq!(repo_id, repo_id_);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_cyphertext() {
        let pool = new_memory_db().await;

        let key = SecretKey::random();

        insert_secret(b"hello", b"world", &key, &pool)
            .await
            .unwrap();

        let v: [u8; 5] = get_secret(b"hello", &key, &pool).await.unwrap();

        assert_eq!(b"world", &v);
    }
}
