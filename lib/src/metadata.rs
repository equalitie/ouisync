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
use sqlx::Row;
use zeroize::Zeroize;

// Metadata keys
const REPOSITORY_ID: &[u8] = b"repository_id";
const PASSWORD_SALT: &[u8] = b"password_salt";
const WRITER_ID: &[u8] = b"writer_id";
const READ_KEY: &[u8] = b"read_key";
const WRITE_KEY: &[u8] = b"write_key";
// We used to have only the ACCESS_KEY which would be either the read key or the write key or a
// dummy (random) array of bytes with the length of the writer key. But that wasn't satisfactory
// because:
//
// 1. The length of the key would leak some information.
// 2. User can't have a separate password for reading and writing, and thus can't plausibly claim
//    they're only a reader if they also have the write access.
// 3. If the ACCESS_KEY was a valid write key, but the repository was in the blind mode, accepting
//    the read token would disable the write access.
const DEPRECATED_ACCESS_KEY: &[u8] = b"access_key"; // read key or write key
const DEVICE_ID: &[u8] = b"device_id";
const READ_KEY_VALIDATOR: &[u8] = b"read_key_validator";

pub(crate) async fn secret_to_key(
    tx: &mut db::Transaction<'_>,
    secret: MasterSecret,
) -> Result<cipher::SecretKey> {
    let key = match secret {
        MasterSecret::SecretKey(key) => key,
        MasterSecret::Password(pwd) => {
            let salt = get_or_generate_password_salt(tx).await?;
            cipher::SecretKey::derive_from_password(pwd.as_ref(), &salt)
        }
    };

    Ok(key)
}

// -------------------------------------------------------------------
// Salt
// -------------------------------------------------------------------
async fn get_or_generate_password_salt(tx: &mut db::Transaction<'_>) -> Result<PasswordSalt> {
    let salt = match get_public(tx, PASSWORD_SALT).await {
        Ok(salt) => salt,
        Err(Error::EntryNotFound) => {
            let salt: PasswordSalt = OsRng.gen();
            set_public(tx, PASSWORD_SALT, &salt).await?;
            salt
        }
        Err(error) => return Err(error),
    };

    Ok(salt)
}

// -------------------------------------------------------------------
// Repository Id
// -------------------------------------------------------------------
pub(crate) async fn get_repository_id(conn: &mut db::Connection) -> Result<RepositoryId> {
    get_public(conn, REPOSITORY_ID).await
}

// -------------------------------------------------------------------
// Writer Id
// -------------------------------------------------------------------
pub(crate) async fn get_writer_id(
    conn: &mut db::Connection,
    master_key: &cipher::SecretKey,
) -> Result<sign::PublicKey> {
    get_secret(conn, WRITER_ID, master_key).await
}

pub(crate) async fn set_writer_id(
    tx: &mut db::Transaction<'_>,
    writer_id: &sign::PublicKey,
    device_id: &DeviceId,
    master_key: &cipher::SecretKey,
) -> Result<()> {
    set_secret(tx, WRITER_ID, writer_id.as_ref(), master_key).await?;
    set_public(tx, DEVICE_ID, device_id.as_ref()).await?;

    Ok(())
}

// -------------------------------------------------------------------
// Replica id
// -------------------------------------------------------------------

// Checks whether the stored device id is the same as the specified one.
pub(crate) async fn check_device_id(
    conn: &mut db::Connection,
    device_id: &DeviceId,
) -> Result<bool> {
    let old_device_id: DeviceId = get_public(conn, DEVICE_ID).await?;
    Ok(old_device_id == *device_id)
}

// -------------------------------------------------------------------
// Access secrets
// -------------------------------------------------------------------
pub(crate) async fn set_access_secrets(
    tx: &mut db::Transaction<'_>,
    secrets: &AccessSecrets,
    master_key: &cipher::SecretKey,
) -> Result<()> {
    set_public(tx, REPOSITORY_ID, secrets.id().as_ref()).await?;

    // Insert a dummy key for plausible deniability.
    let dummy_write_key = sign::SecretKey::random();
    let dummy_read_key = cipher::SecretKey::random();

    match secrets {
        AccessSecrets::Blind { id } => {
            set_secret(tx, READ_KEY, dummy_read_key.as_ref(), master_key).await?;
            set_secret(
                tx,
                READ_KEY_VALIDATOR,
                read_key_validator(id).as_ref(),
                &dummy_read_key,
            )
            .await?;
            set_secret(tx, WRITE_KEY, dummy_write_key.as_ref(), master_key).await?;
        }
        AccessSecrets::Read { id, read_key } => {
            set_secret(tx, READ_KEY, read_key.as_ref(), master_key).await?;
            set_secret(
                tx,
                READ_KEY_VALIDATOR,
                read_key_validator(id).as_ref(),
                read_key,
            )
            .await?;
            set_secret(tx, WRITE_KEY, dummy_write_key.as_ref(), master_key).await?;
        }
        AccessSecrets::Write(secrets) => {
            set_secret(tx, READ_KEY, secrets.read_key.as_ref(), master_key).await?;
            set_secret(
                tx,
                READ_KEY_VALIDATOR,
                read_key_validator(&secrets.id).as_ref(),
                &secrets.read_key,
            )
            .await?;
            set_secret(
                tx,
                WRITE_KEY,
                secrets.write_keys.secret.as_ref(),
                master_key,
            )
            .await?;
        }
    }

    Ok(())
}

pub(crate) async fn get_access_secrets(
    conn: &mut db::Connection,
    master_key: &cipher::SecretKey,
) -> Result<AccessSecrets> {
    let id = get_public(conn, REPOSITORY_ID).await?;

    if let Some(write_keys) = get_write_key(conn, master_key, &id).await? {
        return Ok(AccessSecrets::Write(WriteSecrets::from(write_keys)));
    }

    // No match. Maybe there's the read key?
    if let Some(read_key) = get_read_key(conn, master_key, &id).await? {
        return Ok(AccessSecrets::Read { id, read_key });
    }

    // No read key either, repository shall be open in blind mode.
    Ok(AccessSecrets::Blind { id })
}

async fn get_write_key(
    conn: &mut db::Connection,
    master_key: &cipher::SecretKey,
    id: &RepositoryId,
) -> Result<Option<sign::Keypair>> {
    // Try to interpret it first as the write key.
    let write_key: sign::SecretKey = match get_secret(conn, WRITE_KEY, master_key).await {
        Ok(write_key) => write_key,
        Err(Error::EntryNotFound) =>
        // Let's be backward compatible.
        {
            get_secret(conn, DEPRECATED_ACCESS_KEY, master_key).await?
        }
        Err(error) => return Err(error),
    };

    let write_keys = sign::Keypair::from(write_key);

    let derived_id = RepositoryId::from(write_keys.public);

    if &derived_id == id {
        Ok(Some(write_keys))
    } else {
        Ok(None)
    }
}

async fn get_read_key(
    conn: &mut db::Connection,
    master_key: &cipher::SecretKey,
    id: &RepositoryId,
) -> Result<Option<cipher::SecretKey>> {
    let read_key: cipher::SecretKey = match get_secret(conn, READ_KEY, master_key).await {
        Ok(read_key) => read_key,
        Err(Error::EntryNotFound) => {
            // Let's be backward compatible.
            get_secret(conn, DEPRECATED_ACCESS_KEY, master_key).await?
        }
        Err(error) => return Err(error),
    };

    let key_validator_expected = read_key_validator(id);
    let key_validator_actual: Hash = get_secret(conn, READ_KEY_VALIDATOR, &read_key).await?;

    if key_validator_actual == key_validator_expected {
        // Match - we have read access.
        Ok(Some(read_key))
    } else {
        Ok(None)
    }
}

// -------------------------------------------------------------------
// Public values
// -------------------------------------------------------------------
async fn get_public<T>(conn: &mut db::Connection, id: &[u8]) -> Result<T>
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

async fn set_public(conn: &mut db::Connection, id: &[u8], blob: &[u8]) -> Result<()> {
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
    conn: &mut db::Connection,
    id: &[u8],
    master_key: &cipher::SecretKey,
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
    conn: &mut db::Connection,
    id: &[u8],
    blob: &[u8],
    master_key: &cipher::SecretKey,
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

// -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use tempfile::TempDir;

    async fn setup() -> (TempDir, db::PoolConnection) {
        let (base_dir, pool) = db::create_temp().await.unwrap();
        let conn = pool.acquire().await.unwrap();
        (base_dir, conn)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_plaintext() {
        let (_base_dir, mut conn) = setup().await;

        set_public(&mut conn, b"hello", b"world").await.unwrap();

        let v: [u8; 5] = get_public(&mut conn, b"hello").await.unwrap();

        assert_eq!(b"world", &v);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_cyphertext() {
        let (_base_dir, mut conn) = setup().await;

        let key = cipher::SecretKey::random();

        set_secret(&mut conn, b"hello", b"world", &key)
            .await
            .unwrap();

        let v: [u8; 5] = get_secret(&mut conn, b"hello", &key).await.unwrap();

        assert_eq!(b"world", &v);
    }

    // Using a bad key should not decrypt properly, but also should not cause an error. This is to
    // let user claim plausible deniability in not knowing the real secret key/password.
    #[tokio::test(flavor = "multi_thread")]
    async fn bad_key_is_not_error() {
        let (_base_dir, mut conn) = setup().await;

        let good_key = cipher::SecretKey::random();
        let bad_key = cipher::SecretKey::random();

        set_secret(&mut conn, b"hello", b"world", &good_key)
            .await
            .unwrap();

        let v: [u8; 5] = get_secret(&mut conn, b"hello", &bad_key).await.unwrap();

        assert_ne!(b"world", &v);
    }
}
