use crate::{
    access_control::{AccessSecrets, LocalSecret, WriteSecrets},
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
    secret: LocalSecret,
) -> Result<cipher::SecretKey> {
    let key = match secret {
        LocalSecret::SecretKey(key) => key,
        LocalSecret::Password(pwd) => {
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
// Writer Id
// -------------------------------------------------------------------
pub(crate) async fn get_writer_id(
    conn: &mut db::Connection,
    local_key: Option<&cipher::SecretKey>,
) -> Result<sign::PublicKey> {
    get(conn, WRITER_ID, local_key).await
}

pub(crate) async fn set_writer_id(
    tx: &mut db::Transaction<'_>,
    writer_id: &sign::PublicKey,
    device_id: &DeviceId,
    local_key: Option<&cipher::SecretKey>,
) -> Result<()> {
    set(tx, WRITER_ID, writer_id.as_ref(), local_key).await?;
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
    local_key: Option<&cipher::SecretKey>,
) -> Result<()> {
    set_public(tx, REPOSITORY_ID, secrets.id().as_ref()).await?;

    // Insert a dummy key for plausible deniability.
    let dummy_write_key = sign::SecretKey::random();

    match (local_key, secrets) {
        (Some(local_key), AccessSecrets::Blind { id }) => {
            let dummy_read_key1 = cipher::SecretKey::random();
            let dummy_read_key2 = cipher::SecretKey::random();

            set_secret(tx, READ_KEY, dummy_read_key1.as_ref(), local_key).await?;
            // Using a different dummy key for the validator because we don't want it to validate.
            // If it did validate, the adversary could hold us hostage until we give them local_key
            // that validates.
            set_secret(
                tx,
                READ_KEY_VALIDATOR,
                read_key_validator(id).as_ref(),
                &dummy_read_key2,
            )
            .await?;
            set_secret(tx, WRITE_KEY, dummy_write_key.as_ref(), local_key).await?;
        }
        (Some(local_key), AccessSecrets::Read { id, read_key }) => {
            set_secret(tx, READ_KEY, read_key.as_ref(), local_key).await?;
            set_secret(
                tx,
                READ_KEY_VALIDATOR,
                read_key_validator(id).as_ref(),
                read_key,
            )
            .await?;
            set_secret(tx, WRITE_KEY, dummy_write_key.as_ref(), local_key).await?;
        }
        (Some(local_key), AccessSecrets::Write(secrets)) => {
            set_secret(tx, READ_KEY, secrets.read_key.as_ref(), local_key).await?;
            set_secret(
                tx,
                READ_KEY_VALIDATOR,
                read_key_validator(&secrets.id).as_ref(),
                &secrets.read_key,
            )
            .await?;
            set_secret(tx, WRITE_KEY, secrets.write_keys.secret.as_ref(), local_key).await?;
        }
        (None, AccessSecrets::Blind { id: _ }) => {}
        (None, AccessSecrets::Read { id: _, read_key }) => {
            set_public(tx, READ_KEY, read_key.as_ref()).await?;
        }
        (None, AccessSecrets::Write(secrets)) => {
            set_public(tx, READ_KEY, secrets.read_key.as_ref()).await?;
            set_public(tx, WRITE_KEY, secrets.write_keys.secret.as_ref()).await?;
        }
    }

    Ok(())
}

pub(crate) async fn get_access_secrets(
    conn: &mut db::Connection,
    local_key: Option<&cipher::SecretKey>,
) -> Result<AccessSecrets> {
    let id = get_public(conn, REPOSITORY_ID).await?;

    match get_write_key(conn, local_key, &id).await {
        Ok(write_keys) => return Ok(AccessSecrets::Write(WriteSecrets::from(write_keys))),
        Err(Error::EntryNotFound) => (),
        Err(e) => return Err(e),
    }

    // No match. Maybe there's the read key?
    match get_read_key(conn, local_key, &id).await {
        Ok(read_key) => return Ok(AccessSecrets::Read { id, read_key }),
        Err(Error::EntryNotFound) => (),
        Err(e) => return Err(e),
    }

    // No read key either, repository shall be open in blind mode.
    Ok(AccessSecrets::Blind { id })
}

/// Returns Ok(None) when the key is there but isn't valid.
async fn get_write_key(
    conn: &mut db::Connection,
    local_key: Option<&cipher::SecretKey>,
    id: &RepositoryId,
) -> Result<sign::Keypair> {
    // Try to interpret it first as the write key.
    let write_key: sign::SecretKey = match get(conn, WRITE_KEY, local_key).await {
        Ok(write_key) => write_key,
        Err(Error::EntryNotFound) => {
            // Let's be backward compatible.
            get(conn, DEPRECATED_ACCESS_KEY, local_key).await?
        }
        Err(error) => return Err(error),
    };

    let write_keys = sign::Keypair::from(write_key);

    let derived_id = RepositoryId::from(write_keys.public);

    if &derived_id == id {
        Ok(write_keys)
    } else {
        Err(Error::EntryNotFound)
    }
}

async fn get_read_key(
    conn: &mut db::Connection,
    local_key: Option<&cipher::SecretKey>,
    id: &RepositoryId,
) -> Result<cipher::SecretKey> {
    let read_key: cipher::SecretKey = match get(conn, READ_KEY, local_key).await {
        Ok(read_key) => read_key,
        Err(Error::EntryNotFound) => {
            // Let's be backward compatible.
            get(conn, DEPRECATED_ACCESS_KEY, local_key).await?
        }
        Err(error) => return Err(error),
    };

    if local_key.is_none() {
        return Ok(read_key);
    }

    let key_validator_expected = read_key_validator(id);
    let key_validator_actual: Hash = get_secret(conn, READ_KEY_VALIDATOR, &read_key).await?;

    if key_validator_actual == key_validator_expected {
        // Match - we have read access.
        Ok(read_key)
    } else {
        Err(Error::EntryNotFound)
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
    local_key: &cipher::SecretKey,
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

    local_key.decrypt_no_aead(&nonce, &mut buffer);

    let secret = T::try_from(&buffer).map_err(|_| Error::MalformedData)?;
    buffer.zeroize();

    Ok(secret)
}

async fn set_secret(
    conn: &mut db::Connection,
    id: &[u8],
    blob: &[u8],
    local_key: &cipher::SecretKey,
) -> Result<()> {
    let nonce = make_nonce();

    let mut cypher = blob.to_vec();
    local_key.encrypt_no_aead(&nonce, &mut cypher);

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
async fn get<T>(
    conn: &mut db::Connection,
    id: &[u8],
    local_key: Option<&cipher::SecretKey>,
) -> Result<T>
where
    for<'a> T: TryFrom<&'a [u8]>,
{
    match local_key {
        Some(local_key) => get_secret(conn, id, local_key).await,
        None => get_public(conn, id).await,
    }
}

async fn set(
    conn: &mut db::Connection,
    id: &[u8],
    blob: &[u8],
    local_key: Option<&cipher::SecretKey>,
) -> Result<()> {
    match local_key {
        Some(local_key) => set_secret(conn, id, blob, local_key).await,
        None => set_public(conn, id, blob).await,
    }
}

// -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use assert_matches::assert_matches;
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

    #[tokio::test(flavor = "multi_thread")]
    async fn store_with_local_secret() {
        use crate::access_control::AccessMode;

        for access_mode in [AccessMode::Blind, AccessMode::Read, AccessMode::Write] {
            let (_base_dir, pool) = db::create_temp().await.unwrap();

            let local_secret = cipher::SecretKey::random();

            let access = AccessSecrets::random_write().with_mode(access_mode);
            assert_eq!(access.access_mode(), access_mode);

            let mut tx = pool.begin().await.unwrap();
            set_access_secrets(&mut tx, &access, Some(&local_secret))
                .await
                .unwrap();
            tx.commit().await.unwrap();

            let mut tx = pool.begin().await.unwrap();
            let access_stored = get_access_secrets(&mut tx, Some(&local_secret))
                .await
                .unwrap();
            drop(tx);

            assert_eq!(access, access_stored);

            let mut tx = pool.begin().await.unwrap();
            assert_matches!(
                get_access_secrets(&mut tx, None).await,
                Ok(AccessSecrets::Blind { .. })
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_without_local_secret() {
        use crate::access_control::AccessMode;

        for access_mode in [AccessMode::Blind, AccessMode::Read, AccessMode::Write] {
            let (_base_dir, pool) = db::create_temp().await.unwrap();

            let access = AccessSecrets::random_write().with_mode(access_mode);
            assert_eq!(access.access_mode(), access_mode);

            let mut tx = pool.begin().await.unwrap();
            set_access_secrets(&mut tx, &access, None).await.unwrap();
            tx.commit().await.unwrap();

            let mut tx = pool.begin().await.unwrap();
            let access_stored = get_access_secrets(&mut tx, None).await.unwrap();

            assert_eq!(access_mode, access_stored.access_mode());
        }
    }
}
