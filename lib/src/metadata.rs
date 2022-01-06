use crate::{
    access_control::{AccessSecrets, MasterSecret, WriteSecrets},
    crypto::{
        cipher::{self, AuthTag, Nonce, AUTH_TAG_SIZE},
        sign, PasswordSalt,
    },
    db,
    error::{Error, Result},
    replica_id::ReplicaId,
    repository::RepositoryId,
};
use rand::{rngs::OsRng, Rng};
use sqlx::{Acquire, Row};
use zeroize::Zeroize;

// Metadata keys
const REPOSITORY_ID: &[u8] = b"repository_id";
const PASSWORD_SALT: &[u8] = b"password_salt";
const WRITER_ID: &[u8] = b"writer_id";
const ACCESS_KEY: &[u8] = b"access_key"; // read key or write key
const REPLICA_ID: &[u8] = b"replica_id";

/// Initialize the metadata tables for storing Key:Value pairs.  One table stores plaintext values,
/// the other one stores encrypted ones.
pub(crate) async fn init(conn: &mut db::Connection) -> Result<(), Error> {
    let mut tx = conn.begin().await?;

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
    conn: &mut db::Connection,
) -> Result<cipher::SecretKey> {
    let key = match secret {
        MasterSecret::SecretKey(key) => key,
        MasterSecret::Password(pwd) => {
            let salt = get_or_generate_password_salt(conn).await?;
            cipher::SecretKey::derive_from_password(pwd.as_ref(), &salt)
        }
    };

    Ok(key)
}

// -------------------------------------------------------------------
// Salt
// -------------------------------------------------------------------
async fn get_or_generate_password_salt(conn: &mut db::Connection) -> Result<PasswordSalt> {
    let mut tx = conn.begin().await?;

    let salt = match get_public(PASSWORD_SALT, &mut tx).await {
        Ok(salt) => salt,
        Err(Error::EntryNotFound) => {
            let salt: PasswordSalt = OsRng.gen();
            set_public(PASSWORD_SALT, &salt, &mut tx).await?;
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
pub(crate) async fn get_repository_id(conn: &mut db::Connection) -> Result<RepositoryId> {
    get_public(REPOSITORY_ID, conn).await
}

// -------------------------------------------------------------------
// Writer Id
// -------------------------------------------------------------------
pub(crate) async fn get_writer_id(
    master_key: &cipher::SecretKey,
    conn: &mut db::Connection,
) -> Result<sign::PublicKey> {
    get_secret(WRITER_ID, master_key, conn).await
}

pub(crate) async fn set_writer_id(
    writer_id: &sign::PublicKey,
    replica_id: &ReplicaId,
    master_key: &cipher::SecretKey,
    conn: &mut db::Connection,
) -> Result<()> {
    let mut tx = conn.begin().await?;

    set_secret(WRITER_ID, writer_id.as_ref(), master_key, &mut tx).await?;
    set_public(REPLICA_ID, replica_id.as_ref(), &mut tx).await?;

    tx.commit().await?;

    Ok(())
}

// -------------------------------------------------------------------
// Replica id
// -------------------------------------------------------------------

// Checks whether the stored replica id is the same as the specified one.
pub(crate) async fn check_replica_id(
    replica_id: &ReplicaId,
    conn: &mut db::Connection,
) -> Result<bool> {
    let old_replica_id: ReplicaId = get_public(REPLICA_ID, conn).await?;
    Ok(old_replica_id == *replica_id)
}

// -------------------------------------------------------------------
// Access secrets
// -------------------------------------------------------------------
pub(crate) async fn set_access_secrets(
    secrets: &AccessSecrets,
    master_key: &cipher::SecretKey,
    conn: &mut db::Connection,
) -> Result<()> {
    let mut tx = conn.begin().await?;

    set_public(REPOSITORY_ID, secrets.id().as_ref(), &mut tx).await?;

    match secrets {
        AccessSecrets::Blind { .. } => {
            // Insert a dummy key for plausible deniability.
            let dummy_key = sign::SecretKey::random();
            set_secret(ACCESS_KEY, dummy_key.as_ref(), master_key, &mut tx).await?;
        }
        AccessSecrets::Read { read_key, .. } => {
            set_secret(ACCESS_KEY, read_key.as_ref(), master_key, &mut tx).await?;
        }
        AccessSecrets::Write(secrets) => {
            set_secret(
                ACCESS_KEY,
                secrets.write_keys.secret.as_ref(),
                master_key,
                &mut tx,
            )
            .await?;
        }
    }

    tx.commit().await?;

    Ok(())
}

pub(crate) async fn get_access_secrets(
    master_key: &cipher::SecretKey,
    conn: &mut db::Connection,
) -> Result<AccessSecrets> {
    let id = get_public(REPOSITORY_ID, conn).await?;

    // try to interpret it first as the write key
    let write_key: sign::SecretKey = get_secret(ACCESS_KEY, master_key, conn).await?;
    let write_keys = sign::Keypair::from(write_key);

    let derived_id = RepositoryId::from(write_keys.public);

    if derived_id == id {
        // It was the write key. We have write access.
        Ok(AccessSecrets::Write(WriteSecrets::from(write_keys)))
    } else {
        // It wasn't the write key. Either it's the read key or a dummy key. We can't tell so
        // assume it's the read key for now.
        // unwrap is OK because the two secret key types have the same size.
        let read_key = cipher::SecretKey::try_from(write_keys.secret.as_ref()).unwrap();
        Ok(AccessSecrets::Read { id, read_key })
    }
}

// -------------------------------------------------------------------
// Public values
// -------------------------------------------------------------------
async fn get_public<T>(id: &[u8], conn: &mut db::Connection) -> Result<T>
where
    T: for<'a> TryFrom<&'a [u8]>,
{
    let row = sqlx::query("SELECT value FROM metadata_public WHERE name = ?")
        .bind(id)
        .fetch_optional(conn)
        .await?;
    let row = row.ok_or(Error::EntryNotFound)?;
    let bytes: &[u8] = row.get(0);
    bytes.try_into().map_err(|_| Error::MalformedData)
}

async fn set_public(id: &[u8], blob: &[u8], conn: &mut db::Connection) -> Result<()> {
    sqlx::query("INSERT OR REPLACE INTO metadata_public(name, value) VALUES (?, ?)")
        .bind(id)
        .bind(blob)
        .execute(conn)
        .await?;

    Ok(())
}

// -------------------------------------------------------------------
// Secret values
// -------------------------------------------------------------------
async fn get_secret<T>(
    id: &[u8],
    master_key: &cipher::SecretKey,
    conn: &mut db::Connection,
) -> Result<T>
where
    for<'a> T: TryFrom<&'a [u8]>,
{
    let row = sqlx::query("SELECT nonce, auth_tag, value FROM metadata_secret WHERE name = ?")
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(Error::EntryNotFound)?;

    let nonce: &[u8] = row.get(0);
    let nonce = Nonce::try_from(nonce)?;

    let auth_tag: &[u8] = row.get(1);
    let auth_tag: [u8; AUTH_TAG_SIZE] = auth_tag.try_into()?;
    let auth_tag = AuthTag::from(auth_tag);

    let mut buffer: Vec<_> = row.get(2);

    master_key.decrypt(nonce, id, &mut buffer, &auth_tag)?;

    let secret = T::try_from(&buffer).map_err(|_| Error::MalformedData)?;
    buffer.zeroize();

    Ok(secret)
}

async fn set_secret(
    id: &[u8],
    blob: &[u8],
    master_key: &cipher::SecretKey,
    conn: &mut db::Connection,
) -> Result<()> {
    let nonce = make_nonce();

    let mut cypher = blob.to_vec();
    let auth_tag = master_key.encrypt(nonce, id, &mut cypher)?;

    sqlx::query(
        "INSERT OR REPLACE INTO metadata_secret(name, nonce, auth_tag, value)
            VALUES (?, ?, ?, ?)",
    )
    .bind(id)
    .bind(&nonce[..])
    .bind(&auth_tag[..])
    .bind(&cypher)
    .execute(conn)
    .await?;

    Ok(())
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
    use sqlx::Connection;

    async fn new_memory_db() -> db::Connection {
        let mut conn = db::Connection::connect(":memory:").await.unwrap();
        init(&mut conn).await.unwrap();
        conn
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_plaintext() {
        let mut conn = new_memory_db().await;

        set_public(b"hello", b"world", &mut conn).await.unwrap();

        let v: [u8; 5] = get_public(b"hello", &mut conn).await.unwrap();

        assert_eq!(b"world", &v);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_cyphertext() {
        let mut conn = new_memory_db().await;

        let key = cipher::SecretKey::random();

        set_secret(b"hello", b"world", &key, &mut conn)
            .await
            .unwrap();

        let v: [u8; 5] = get_secret(b"hello", &key, &mut conn).await.unwrap();

        assert_eq!(b"world", &v);
    }
}
