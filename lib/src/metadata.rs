use crate::{
    access_control::{Access, AccessSecrets, LocalSecret, WriteSecrets},
    crypto::{
        cipher::{self, Nonce},
        sign, Hash, Password, PasswordSalt,
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

pub(crate) async fn password_to_key(
    tx: &mut db::Transaction,
    password: &Password,
) -> Result<cipher::SecretKey> {
    let salt = get_or_generate_password_salt(tx).await?;
    Ok(cipher::SecretKey::derive_from_password(
        password.as_ref(),
        &salt,
    ))
}

pub(crate) async fn secret_to_key(
    tx: &mut db::Transaction,
    secret: LocalSecret,
) -> Result<cipher::SecretKey> {
    match secret {
        LocalSecret::Password(password) => password_to_key(tx, &password).await,
        LocalSecret::SecretKey(key) => Ok(key),
    }
}

// -------------------------------------------------------------------
// Salt
// -------------------------------------------------------------------
async fn get_or_generate_password_salt(tx: &mut db::Transaction) -> Result<PasswordSalt> {
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
    tx: &mut db::Transaction,
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
async fn set_public_read_key(tx: &mut db::Transaction, read_key: &cipher::SecretKey) -> Result<()> {
    set_public(tx, READ_KEY, read_key.as_ref()).await
}

async fn set_secret_read_key(
    tx: &mut db::Transaction,
    id: &RepositoryId,
    read_key: &cipher::SecretKey,
    local_key: &cipher::SecretKey,
) -> Result<()> {
    set_secret(tx, READ_KEY, read_key.as_ref(), local_key).await?;
    set_secret(
        tx,
        READ_KEY_VALIDATOR,
        read_key_validator(id).as_ref(),
        read_key,
    )
    .await
}

pub(crate) async fn set_read_key(
    tx: &mut db::Transaction,
    id: &RepositoryId,
    read_key: &cipher::SecretKey,
    local_key: Option<&cipher::SecretKey>,
) -> Result<()> {
    if let Some(local_key) = local_key {
        remove_public_read_key(tx).await?;
        set_secret_read_key(tx, id, read_key, local_key).await
    } else {
        set_public_read_key(tx, read_key).await?;
        remove_secret_write_key(tx).await
    }
}

async fn remove_public_read_key(tx: &mut db::Connection) -> Result<()> {
    sqlx::query("DELETE FROM metadata_public WHERE name = ?")
        .bind(READ_KEY)
        .execute(tx)
        .await?;
    Ok(())
}

async fn remove_secret_read_key(tx: &mut db::Transaction) -> Result<()> {
    let dummy_id = RepositoryId::random();
    let dummy_local_key = cipher::SecretKey::random();
    let dummy_read_key = cipher::SecretKey::random();

    set_secret(tx, READ_KEY, dummy_read_key.as_ref(), &dummy_local_key).await?;
    set_secret(
        tx,
        READ_KEY_VALIDATOR,
        read_key_validator(&dummy_id).as_ref(),
        &dummy_read_key,
    )
    .await?;

    Ok(())
}

pub(crate) async fn remove_read_key(tx: &mut db::Transaction) -> Result<()> {
    remove_public_read_key(tx).await?;
    remove_secret_read_key(tx).await
}

// ------------------------------

async fn set_public_write_key(tx: &mut db::Transaction, secrets: &WriteSecrets) -> Result<()> {
    set_public(tx, WRITE_KEY, secrets.write_keys.secret.as_ref()).await
}

async fn set_secret_write_key(
    tx: &mut db::Transaction,
    secrets: &WriteSecrets,
    local_key: &cipher::SecretKey,
) -> Result<()> {
    set_secret(tx, WRITE_KEY, secrets.write_keys.secret.as_ref(), local_key).await
}

pub(crate) async fn set_write_key(
    tx: &mut db::Transaction,
    secrets: &WriteSecrets,
    local_key: Option<&cipher::SecretKey>,
) -> Result<()> {
    if let Some(local_key) = local_key {
        remove_public_write_key(tx).await?;
        set_secret_write_key(tx, secrets, local_key).await
    } else {
        set_public_write_key(tx, secrets).await?;
        remove_secret_write_key(tx).await
    }
}

async fn remove_public_write_key(tx: &mut db::Connection) -> Result<()> {
    sqlx::query("DELETE FROM metadata_public WHERE name = ?")
        .bind(WRITE_KEY)
        .execute(tx)
        .await?;
    Ok(())
}

async fn remove_secret_write_key(tx: &mut db::Transaction) -> Result<()> {
    let dummy_local_key = cipher::SecretKey::random();
    let dummy_write_key = sign::SecretKey::random();
    set_secret(tx, WRITE_KEY, dummy_write_key.as_ref(), &dummy_local_key).await
}

pub(crate) async fn remove_write_key(tx: &mut db::Transaction) -> Result<()> {
    remove_public_write_key(tx).await?;
    remove_secret_write_key(tx).await
}

// ------------------------------

pub(crate) async fn requires_local_password_for_reading(conn: &mut db::Connection) -> Result<bool> {
    match get_public::<cipher::SecretKey>(conn, READ_KEY).await {
        Ok(_) => return Ok(false),
        Err(Error::EntryNotFound) => (),
        Err(err) => return Err(err),
    }

    match get_public::<sign::SecretKey>(conn, WRITE_KEY).await {
        Ok(_) => Ok(false),
        Err(Error::EntryNotFound) => Ok(true),
        Err(err) => Err(err),
    }
}

pub(crate) async fn requires_local_password_for_writing(conn: &mut db::Connection) -> Result<bool> {
    match get_public::<sign::SecretKey>(conn, WRITE_KEY).await {
        Ok(_) => Ok(false),
        Err(Error::EntryNotFound) => Ok(true),
        Err(err) => Err(err),
    }
}

pub(crate) async fn initialize_access_secrets(
    tx: &mut db::Transaction,
    access: &Access,
) -> Result<()> {
    set_public(tx, REPOSITORY_ID, access.id().as_ref()).await?;
    set_access(tx, access).await
}

pub(crate) async fn set_access(tx: &mut db::Transaction, access: &Access) -> Result<()> {
    match access {
        Access::Blind { .. } => {
            remove_public_read_key(tx).await?;
            remove_secret_read_key(tx).await?;
            remove_public_write_key(tx).await?;
            remove_secret_write_key(tx).await?;
        }
        Access::ReadUnlocked { id: _, read_key } => {
            set_public_read_key(tx, read_key).await?;
            remove_secret_read_key(tx).await?;
            remove_public_write_key(tx).await?;
            remove_secret_write_key(tx).await?;
        }
        Access::ReadLocked {
            id,
            local_key,
            read_key,
        } => {
            remove_public_read_key(tx).await?;
            set_secret_read_key(tx, id, read_key, local_key).await?;
            remove_public_write_key(tx).await?;
            remove_secret_write_key(tx).await?;
        }
        Access::WriteUnlocked { secrets } => {
            set_public_read_key(tx, &secrets.read_key).await?;
            remove_secret_read_key(tx).await?;
            set_public_write_key(tx, secrets).await?;
            remove_secret_write_key(tx).await?;
        }
        Access::WriteLocked {
            local_read_key,
            local_write_key,
            secrets,
        } => {
            remove_public_read_key(tx).await?;
            set_secret_read_key(tx, &secrets.id, &secrets.read_key, local_read_key).await?;
            remove_public_write_key(tx).await?;
            set_secret_write_key(tx, secrets, local_write_key).await?;
        }
        Access::WriteLockedReadUnlocked {
            local_write_key,
            secrets,
        } => {
            set_public_read_key(tx, &secrets.read_key).await?;
            remove_secret_read_key(tx).await?;
            remove_public_write_key(tx).await?;
            set_secret_write_key(tx, secrets, local_write_key).await?;
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

async fn set_public(tx: &mut db::Transaction, id: &[u8], blob: &[u8]) -> Result<()> {
    sqlx::query("INSERT OR REPLACE INTO metadata_public(name, value) VALUES (?, ?)")
        .bind(id)
        .bind(blob)
        .execute(&mut **tx)
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
    tx: &mut db::Transaction,
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
    .execute(&mut **tx)
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
    tx: &mut db::Transaction,
    id: &[u8],
    blob: &[u8],
    local_key: Option<&cipher::SecretKey>,
) -> Result<()> {
    match local_key {
        Some(local_key) => set_secret(tx, id, blob, local_key).await,
        None => set_public(tx, id, blob).await,
    }
}

// -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use tempfile::TempDir;

    async fn setup() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_plaintext() {
        let (_base_dir, pool) = setup().await;
        let mut tx = pool.begin().await.unwrap();

        set_public(&mut tx, b"hello", b"world").await.unwrap();

        let v: [u8; 5] = get_public(&mut tx, b"hello").await.unwrap();

        assert_eq!(b"world", &v);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_cyphertext() {
        let (_base_dir, pool) = setup().await;
        let mut tx = pool.begin().await.unwrap();

        let key = cipher::SecretKey::random();

        set_secret(&mut tx, b"hello", b"world", &key).await.unwrap();

        let v: [u8; 5] = get_secret(&mut tx, b"hello", &key).await.unwrap();

        assert_eq!(b"world", &v);
    }

    // Using a bad key should not decrypt properly, but also should not cause an error. This is to
    // let user claim plausible deniability in not knowing the real secret key/password.
    #[tokio::test(flavor = "multi_thread")]
    async fn bad_key_is_not_error() {
        let (_base_dir, pool) = setup().await;
        let mut tx = pool.begin().await.unwrap();

        let good_key = cipher::SecretKey::random();
        let bad_key = cipher::SecretKey::random();

        set_secret(&mut tx, b"hello", b"world", &good_key)
            .await
            .unwrap();

        let v: [u8; 5] = get_secret(&mut tx, b"hello", &bad_key).await.unwrap();

        assert_ne!(b"world", &v);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_restore() {
        let accesses = [
            Access::Blind {
                id: RepositoryId::random(),
            },
            Access::ReadUnlocked {
                id: RepositoryId::random(),
                read_key: cipher::SecretKey::random(),
            },
            Access::ReadLocked {
                id: RepositoryId::random(),
                local_key: cipher::SecretKey::random(),
                read_key: cipher::SecretKey::random(),
            },
            Access::WriteUnlocked {
                secrets: WriteSecrets::random(),
            },
            Access::WriteLocked {
                local_read_key: cipher::SecretKey::random(),
                local_write_key: cipher::SecretKey::random(),
                secrets: WriteSecrets::random(),
            },
            Access::WriteLockedReadUnlocked {
                local_write_key: cipher::SecretKey::random(),
                secrets: WriteSecrets::random(),
            },
        ];

        for access in accesses {
            let (_base_dir, pool) = db::create_temp().await.unwrap();

            let mut tx = pool.begin().await.unwrap();
            initialize_access_secrets(&mut tx, &access).await.unwrap();
            tx.commit().await.unwrap();

            let local_key = access.highest_local_key();

            let mut conn = pool.acquire().await.unwrap();

            let access_secrets = get_access_secrets(&mut conn, local_key).await.unwrap();

            assert_eq!(access.secrets(), access_secrets);
        }
    }
}
