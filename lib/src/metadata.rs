use crate::{
    access_control::{AccessSecrets, MasterSecret, WriteSecrets},
    crypto::{
        cipher::{self, Nonce},
        sign, Hash, PasswordSalt,
    },
    db,
    device_id::DeviceId,
    error::{Error, Result},
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
const DEVICE_ID: &[u8] = b"device_id";
const READ_KEY_VALIDATOR: &[u8] = b"read_key_validator";

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
    device_id: &DeviceId,
    master_key: &cipher::SecretKey,
    conn: &mut db::Connection,
) -> Result<()> {
    let mut tx = conn.begin().await?;

    set_secret(WRITER_ID, writer_id.as_ref(), master_key, &mut tx).await?;
    set_public(DEVICE_ID, device_id.as_ref(), &mut tx).await?;

    tx.commit().await?;

    Ok(())
}

// -------------------------------------------------------------------
// Replica id
// -------------------------------------------------------------------

// Checks whether the stored device id is the same as the specified one.
pub(crate) async fn check_device_id(
    device_id: &DeviceId,
    conn: &mut db::Connection,
) -> Result<bool> {
    let old_device_id: DeviceId = get_public(DEVICE_ID, conn).await?;
    Ok(old_device_id == *device_id)
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
        AccessSecrets::Blind { id } => {
            // Insert a dummy key for plausible deniability.
            let dummy_write_key = sign::SecretKey::random();
            let dummy_read_key = cipher::SecretKey::random();

            set_secret(ACCESS_KEY, dummy_write_key.as_ref(), master_key, &mut tx).await?;
            set_secret(
                READ_KEY_VALIDATOR,
                read_key_validator(id).as_ref(),
                &dummy_read_key,
                &mut tx,
            )
            .await?;
        }
        AccessSecrets::Read { id, read_key } => {
            set_secret(ACCESS_KEY, read_key.as_ref(), master_key, &mut tx).await?;
            set_secret(
                READ_KEY_VALIDATOR,
                read_key_validator(id).as_ref(),
                read_key,
                &mut tx,
            )
            .await?;
        }
        AccessSecrets::Write(secrets) => {
            set_secret(
                ACCESS_KEY,
                secrets.write_keys.secret.as_ref(),
                master_key,
                &mut tx,
            )
            .await?;
            set_secret(
                READ_KEY_VALIDATOR,
                read_key_validator(&secrets.id).as_ref(),
                &secrets.read_key,
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

    // Try to interpret it first as the write key.
    let write_key: sign::SecretKey = get_secret(ACCESS_KEY, master_key, conn).await?;
    let write_keys = sign::Keypair::from(write_key);

    let derived_id = RepositoryId::from(write_keys.public);

    if derived_id == id {
        // Match - we have write access.
        Ok(AccessSecrets::Write(WriteSecrets::from(write_keys)))
    } else {
        // No match. Maybe it's the read key?
        let read_key = sign_key_to_cipher_key(&write_keys.secret);
        let key_validator_expected = read_key_validator(&id);
        let key_validator_actual: Hash = get_secret(READ_KEY_VALIDATOR, &read_key, conn).await?;

        if key_validator_actual == key_validator_expected {
            // Match - we have read access.
            Ok(AccessSecrets::Read { id, read_key })
        } else {
            // No match. The key is just a dummy and we have only blind access.
            Ok(AccessSecrets::Blind { id })
        }
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
    let row = sqlx::query("SELECT nonce, value FROM metadata_secret WHERE name = ?")
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(Error::EntryNotFound)?;

    let nonce: &[u8] = row.get(0);
    let nonce = Nonce::try_from(nonce)?;

    let mut buffer: Vec<_> = row.get(1);

    master_key.decrypt_no_aead(&nonce, &mut buffer);

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
    master_key.encrypt_no_aead(&nonce, &mut cypher);

    sqlx::query(
        "INSERT OR REPLACE INTO metadata_secret(name, nonce, value)
            VALUES (?, ?, ?)",
    )
    .bind(id)
    .bind(&nonce[..])
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

// String used to validate the read key
fn read_key_validator(id: &RepositoryId) -> Hash {
    id.salted_hash(b"ouisync read key validator")
}

// Convert signing key to encryption key.
fn sign_key_to_cipher_key(sk: &sign::SecretKey) -> cipher::SecretKey {
    // unwrap is ok because the two key types have the same length.
    sk.as_ref().as_slice().try_into().unwrap()
}

// -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn new_memory_db() -> db::Connection {
        let mut conn = db::open_or_create(&db::Store::Temporary)
            .await
            .unwrap()
            .acquire()
            .await
            .unwrap()
            .detach();
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

    // Using a bad key should not decrypt properly, but also should not cause an error. This is to
    // let user claim plausible deniability in not knowing the real secret key/password.
    #[tokio::test(flavor = "multi_thread")]
    async fn bad_key_is_not_error() {
        let mut conn = new_memory_db().await;

        let good_key = cipher::SecretKey::random();
        let bad_key = cipher::SecretKey::random();

        set_secret(b"hello", b"world", &good_key, &mut conn)
            .await
            .unwrap();

        let v: [u8; 5] = get_secret(b"hello", &bad_key, &mut conn).await.unwrap();

        assert_ne!(b"world", &v);
    }
}
