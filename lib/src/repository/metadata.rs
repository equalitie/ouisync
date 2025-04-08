use crate::{
    access_control::{Access, AccessSecrets, LocalSecret, SetLocalSecret, WriteSecrets},
    crypto::{
        cipher::{self, Nonce},
        sign, Hash, Password, PasswordSalt,
    },
    db::{self, DatabaseId},
    device_id::DeviceId,
    protocol::RepositoryId,
    store::Error as StoreError,
};
use rand::{rngs::OsRng, Rng};
use sqlx::Row;
use std::{borrow::Cow, fmt, time::Duration};
use tracing::instrument;
use zeroize::Zeroize;

// Metadata keys
const REPOSITORY_ID: &[u8] = b"repository_id";
// Note that we don't do anything with these salts other than storing them for the user so they can
// use them for SecretKey derivation. Storing them here (as opposed to having the user store it
// outside of this repository database) ensures that when the database is moved to another device,
// the same password can still unlock it.
const READ_PASSWORD_SALT: &[u8] = b"read_password_salt";
const WRITE_PASSWORD_SALT: &[u8] = b"write_password_salt";
const WRITER_ID: &[u8] = b"writer_id";
const READ_KEY: &[u8] = b"read_key";
const WRITE_KEY: &[u8] = b"write_key";
const DATABASE_ID: &[u8] = b"database_id";

const DEVICE_ID: &[u8] = b"device_id";
const READ_KEY_VALIDATOR: &[u8] = b"read_key_validator";

const QUOTA: &[u8] = b"quota";
const BLOCK_EXPIRATION: &[u8] = b"block_expiration";

// Support for data migrations.
const DATA_VERSION: &[u8] = b"data_version";

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

// We used to have a single salt that was used when the user used a password to open a repository.
// We no longer deal with passwords in ouisync_lib but leave the password hashing to the library
// user (we provide them with functions to do it) and instead of a single salt we can store two for
// them: one for the read password and one for the write password.
const DEPRECATED_PASSWORD_SALT: &[u8] = b"password_salt";

// -------------------------------------------------------------------
// Accessor for user-defined metadata
// -------------------------------------------------------------------
pub struct Metadata {
    db: db::Pool,
}

impl Metadata {
    pub(crate) fn new(db: db::Pool) -> Self {
        Self { db }
    }

    #[instrument(skip(self), fields(value))]
    pub async fn get<T>(&self, name: &str) -> Result<Option<T>, StoreError>
    where
        T: MetadataGet + fmt::Debug,
    {
        let mut conn = self.db.acquire().await?;
        get_public(&mut conn, name.as_bytes()).await
    }

    pub async fn set<'a, T>(&self, name: &'a str, value: T) -> Result<(), StoreError>
    where
        T: MetadataSet<'a> + fmt::Debug,
    {
        let mut tx = self.write().await?;
        tx.set(name, value).await?;
        tx.commit().await?;

        Ok(())
    }

    pub async fn remove(&self, name: &str) -> Result<(), StoreError> {
        let mut tx = self.write().await?;
        tx.remove(name).await?;
        tx.commit().await?;

        Ok(())
    }

    pub async fn write(&self) -> Result<MetadataWriter, StoreError> {
        Ok(MetadataWriter {
            tx: self.db.begin_write().await?,
        })
    }
}

pub struct MetadataWriter {
    tx: db::WriteTransaction,
}

impl MetadataWriter {
    pub async fn get<T>(&mut self, name: &str) -> Result<Option<T>, StoreError>
    where
        T: MetadataGet + fmt::Debug,
    {
        get_public(&mut self.tx, name.as_bytes()).await
    }

    pub async fn set<'a, T>(&mut self, name: &'a str, value: T) -> Result<(), StoreError>
    where
        T: MetadataSet<'a> + fmt::Debug,
    {
        set_public(&mut self.tx, name.as_bytes(), value).await
    }

    pub async fn remove(&mut self, name: &str) -> Result<(), StoreError> {
        remove_public(&mut self.tx, name.as_bytes()).await
    }

    pub async fn commit(self) -> Result<(), StoreError> {
        self.tx.commit().await?;
        Ok(())
    }
}

// -------------------------------------------------------------------
// Password
// -------------------------------------------------------------------
pub(crate) enum KeyType {
    Read,
    Write,
}

pub(crate) async fn password_to_key(
    tx: &mut db::WriteTransaction,
    key_type: KeyType,
    password: &Password,
) -> Result<cipher::SecretKey, StoreError> {
    let salt = get_password_salt(tx, key_type).await?;

    Ok(cipher::SecretKey::derive_from_password(
        password.as_ref(),
        &salt,
    ))
}

async fn secret_to_key<'a>(
    tx: &mut db::WriteTransaction,
    key_type: KeyType,
    secret: &'a LocalSecret,
) -> Result<Cow<'a, cipher::SecretKey>, StoreError> {
    match secret {
        LocalSecret::Password(password) => password_to_key(tx, key_type, password)
            .await
            .map(Cow::Owned),
        LocalSecret::SecretKey(key) => Ok(Cow::Borrowed(key)),
    }
}

pub(crate) fn secret_to_key_and_salt(
    secret: &'_ SetLocalSecret,
) -> (Cow<'_, cipher::SecretKey>, Cow<'_, PasswordSalt>) {
    match secret {
        SetLocalSecret::Password(password) => {
            let salt = PasswordSalt::random();
            let key = cipher::SecretKey::derive_from_password(password.as_ref(), &salt);
            (Cow::Owned(key), Cow::Owned(salt))
        }
        SetLocalSecret::KeyAndSalt { key, salt } => (Cow::Borrowed(key), Cow::Borrowed(salt)),
    }
}

// -------------------------------------------------------------------
// Database ID
// -------------------------------------------------------------------
pub(crate) async fn get_or_generate_database_id(db: &db::Pool) -> Result<DatabaseId, StoreError> {
    let mut tx = db.begin_write().await?;
    let database_id = match get_public_blob(&mut tx, DATABASE_ID).await {
        Ok(Some(database_id)) => database_id,
        Ok(None) => {
            let database_id: DatabaseId = OsRng.gen();
            set_public_blob(&mut tx, DATABASE_ID, &database_id).await?;
            tx.commit().await?;
            database_id
        }
        Err(error) => return Err(error),
    };

    Ok(database_id)
}

// -------------------------------------------------------------------
// Writer Id
// -------------------------------------------------------------------
pub(crate) async fn get_writer_id(
    conn: &mut db::Connection,
    local_key: Option<&cipher::SecretKey>,
) -> Result<Option<sign::PublicKey>, StoreError> {
    get_blob(conn, WRITER_ID, local_key).await
}

pub(crate) async fn set_writer_id(
    tx: &mut db::WriteTransaction,
    writer_id: &sign::PublicKey,
    local_key: Option<&cipher::SecretKey>,
) -> Result<(), StoreError> {
    set_blob(tx, WRITER_ID, writer_id, local_key).await?;
    Ok(())
}

// TODO: Writer IDs are currently practically just UUIDs with no real security (any replica with a
// write access may impersonate any other replica).
pub(crate) fn generate_writer_id() -> sign::PublicKey {
    sign::Keypair::random().public_key()
}

pub(crate) async fn get_or_generate_writer_id(
    tx: &mut db::WriteTransaction,
    local_key: Option<&cipher::SecretKey>,
) -> Result<sign::PublicKey, StoreError> {
    let writer_id = if let Some(writer_id) = get_writer_id(tx, local_key).await? {
        writer_id
    } else {
        let writer_id = generate_writer_id();
        set_writer_id(tx, &writer_id, local_key).await?;
        writer_id
    };

    Ok(writer_id)
}

// -------------------------------------------------------------------
// Device id
// -------------------------------------------------------------------

// Checks whether the stored device id is the same as the specified one.
pub(crate) async fn check_device_id(
    conn: &mut db::Connection,
    device_id: &DeviceId,
) -> Result<bool, StoreError> {
    let old_device_id: Option<DeviceId> = get_public_blob(conn, DEVICE_ID).await?;
    Ok(old_device_id.as_ref() == Some(device_id))
}

pub(crate) async fn set_device_id(
    tx: &mut db::WriteTransaction,
    device_id: &DeviceId,
) -> Result<(), StoreError> {
    set_public_blob(tx, DEVICE_ID, device_id).await?;
    Ok(())
}

// -------------------------------------------------------------------
// Access secrets
// -------------------------------------------------------------------
pub(super) async fn get_repository_id(
    conn: &mut db::Connection,
) -> Result<RepositoryId, StoreError> {
    // Repository id should always exist. If not indicates a corrupted db.
    get_public_blob(conn, REPOSITORY_ID)
        .await?
        .ok_or(StoreError::MalformedData)
}

async fn set_public_read_key(
    tx: &mut db::WriteTransaction,
    read_key: &cipher::SecretKey,
) -> Result<(), StoreError> {
    set_public_blob(tx, READ_KEY, read_key).await
}

async fn set_secret_read_key(
    tx: &mut db::WriteTransaction,
    id: &RepositoryId,
    read_key: &cipher::SecretKey,
    local_secret_key: &cipher::SecretKey,
    local_salt: &PasswordSalt,
) -> Result<(), StoreError> {
    set_secret_blob(tx, READ_KEY, read_key, local_secret_key).await?;
    set_secret_blob(tx, READ_KEY_VALIDATOR, read_key_validator(id), read_key).await?;
    set_password_salt(tx, KeyType::Read, local_salt).await
}

pub(crate) async fn set_read_key(
    tx: &mut db::WriteTransaction,
    id: &RepositoryId,
    read_key: &cipher::SecretKey,
    local: Option<(&cipher::SecretKey, &PasswordSalt)>,
) -> Result<(), StoreError> {
    if let Some((local_secret_key, local_salt)) = local {
        remove_public_read_key(tx).await?;
        set_secret_read_key(tx, id, read_key, local_secret_key, local_salt).await
    } else {
        set_public_read_key(tx, read_key).await?;
        obfuscate_secret_read_key(tx).await
    }
}

async fn remove_public_read_key(tx: &mut db::WriteTransaction) -> Result<(), StoreError> {
    remove_public(tx, READ_KEY).await
}

async fn obfuscate_secret_read_key(tx: &mut db::WriteTransaction) -> Result<(), StoreError> {
    let dummy_id = RepositoryId::from(sign::Keypair::random().public_key());
    let dummy_local_key = cipher::SecretKey::random();
    let dummy_read_key = cipher::SecretKey::random();

    set_secret_blob(tx, READ_KEY, &dummy_read_key, &dummy_local_key).await?;
    set_secret_blob(
        tx,
        READ_KEY_VALIDATOR,
        read_key_validator(&dummy_id),
        &dummy_read_key,
    )
    .await?;

    obfuscate_read_password_salt(tx).await?;

    Ok(())
}

pub(crate) async fn remove_read_key(tx: &mut db::WriteTransaction) -> Result<(), StoreError> {
    remove_public_read_key(tx).await?;
    obfuscate_secret_read_key(tx).await
}

// ------------------------------

async fn set_public_write_key(
    tx: &mut db::WriteTransaction,
    secrets: &WriteSecrets,
) -> Result<(), StoreError> {
    set_public_blob(tx, WRITE_KEY, secrets.write_keys.to_bytes()).await
}

async fn set_secret_write_key(
    tx: &mut db::WriteTransaction,
    secrets: &WriteSecrets,
    local_secret_key: &cipher::SecretKey,
    local_salt: &PasswordSalt,
) -> Result<(), StoreError> {
    set_secret_blob(
        tx,
        WRITE_KEY,
        secrets.write_keys.to_bytes(),
        local_secret_key,
    )
    .await?;
    set_password_salt(tx, KeyType::Write, local_salt).await
}

pub(crate) async fn set_write_key(
    tx: &mut db::WriteTransaction,
    secrets: &WriteSecrets,
    local: Option<(&cipher::SecretKey, &PasswordSalt)>,
) -> Result<(), StoreError> {
    if let Some((local_secret_key, local_salt)) = local {
        remove_public_write_key(tx).await?;
        set_secret_write_key(tx, secrets, local_secret_key, local_salt).await
    } else {
        set_public_write_key(tx, secrets).await?;
        obfuscate_secret_write_key(tx).await
    }
}

async fn remove_public_write_key(tx: &mut db::WriteTransaction) -> Result<(), StoreError> {
    remove_public(tx, WRITE_KEY).await
}

async fn obfuscate_secret_write_key(tx: &mut db::WriteTransaction) -> Result<(), StoreError> {
    let dummy_local_key = cipher::SecretKey::random();
    let dummy_write_key = sign::Keypair::random().to_bytes();
    set_secret_blob(tx, WRITE_KEY, &dummy_write_key, &dummy_local_key).await?;
    obfuscate_write_password_salt(tx).await
}

pub(crate) async fn remove_write_key(tx: &mut db::WriteTransaction) -> Result<(), StoreError> {
    remove_public_write_key(tx).await?;
    obfuscate_secret_write_key(tx).await
}

// ------------------------------

pub(crate) async fn get_password_salt(
    tx: &mut db::WriteTransaction,
    key_type: KeyType,
) -> Result<PasswordSalt, StoreError> {
    migrate_to_separate_password_salts(tx).await?;

    match key_type {
        KeyType::Read => get_public_blob(tx, READ_PASSWORD_SALT).await?,
        KeyType::Write => get_public_blob(tx, WRITE_PASSWORD_SALT).await?,
    }
    // Salts should always be present
    .ok_or(StoreError::MalformedData)
}

async fn set_password_salt(
    tx: &mut db::WriteTransaction,
    key_type: KeyType,
    salt: &PasswordSalt,
) -> Result<(), StoreError> {
    migrate_to_separate_password_salts(tx).await?;
    match key_type {
        KeyType::Read => set_public_blob(tx, READ_PASSWORD_SALT, salt).await,
        KeyType::Write => set_public_blob(tx, WRITE_PASSWORD_SALT, salt).await,
    }
}

async fn obfuscate_read_password_salt(tx: &mut db::WriteTransaction) -> Result<(), StoreError> {
    migrate_to_separate_password_salts(tx).await?;
    let dummy_salt = PasswordSalt::random();
    set_public_blob(tx, READ_PASSWORD_SALT, &dummy_salt).await
}

async fn obfuscate_write_password_salt(tx: &mut db::WriteTransaction) -> Result<(), StoreError> {
    migrate_to_separate_password_salts(tx).await?;
    let dummy_salt = PasswordSalt::random();
    set_public_blob(tx, WRITE_PASSWORD_SALT, &dummy_salt).await
}

async fn migrate_to_separate_password_salts(
    tx: &mut db::WriteTransaction,
) -> Result<(), StoreError> {
    let single_salt: PasswordSalt = match get_public_blob(tx, DEPRECATED_PASSWORD_SALT).await {
        Ok(Some(salt)) => salt,
        Ok(None) => {
            // Single salt has already been migrated.
            return Ok(());
        }
        Err(error) => return Err(error),
    };

    set_public_blob(tx, READ_PASSWORD_SALT, &single_salt).await?;
    set_public_blob(tx, WRITE_PASSWORD_SALT, &single_salt).await?;
    remove_public(tx, DEPRECATED_PASSWORD_SALT).await
}

// ------------------------------

pub(crate) async fn requires_local_secret_for_reading(
    conn: &mut db::Connection,
) -> Result<bool, StoreError> {
    match get_public_blob::<cipher::SecretKey>(conn, READ_KEY).await {
        Ok(Some(_)) => return Ok(false),
        Ok(None) => (),
        Err(err) => return Err(err),
    }

    match get_public_blob::<sign::Keypair>(conn, WRITE_KEY).await {
        Ok(Some(_)) => Ok(false),
        Ok(None) => Ok(true),
        Err(err) => Err(err),
    }
}

pub(crate) async fn requires_local_secret_for_writing(
    conn: &mut db::Connection,
) -> Result<bool, StoreError> {
    match get_public_blob::<sign::Keypair>(conn, WRITE_KEY).await {
        Ok(Some(_)) => Ok(false),
        Ok(None) => Ok(true),
        Err(err) => Err(err),
    }
}

pub(crate) async fn initialize_access_secrets<'a>(
    tx: &mut db::WriteTransaction,
    access: &'a Access,
) -> Result<LocalKeys<'a>, StoreError> {
    set_public_blob(tx, REPOSITORY_ID, access.id()).await?;
    set_access(tx, access).await
}

pub(crate) async fn set_access<'a>(
    tx: &mut db::WriteTransaction,
    access: &'a Access,
) -> Result<LocalKeys<'a>, StoreError> {
    match access {
        Access::Blind { .. } => {
            remove_public_read_key(tx).await?;
            obfuscate_secret_read_key(tx).await?;
            remove_public_write_key(tx).await?;
            obfuscate_secret_write_key(tx).await?;

            Ok(LocalKeys {
                read: None,
                write: None,
            })
        }
        Access::ReadUnlocked { id: _, read_key } => {
            set_public_read_key(tx, read_key).await?;
            obfuscate_secret_read_key(tx).await?;
            remove_public_write_key(tx).await?;
            obfuscate_secret_write_key(tx).await?;

            Ok(LocalKeys {
                read: None,
                write: None,
            })
        }
        Access::ReadLocked {
            id,
            local_secret,
            read_key,
        } => {
            let (local_secret_key, local_salt) = secret_to_key_and_salt(local_secret);

            remove_public_read_key(tx).await?;
            set_secret_read_key(tx, id, read_key, &local_secret_key, &local_salt).await?;
            remove_public_write_key(tx).await?;
            obfuscate_secret_write_key(tx).await?;

            Ok(LocalKeys {
                read: Some(local_secret_key),
                write: None,
            })
        }
        Access::WriteUnlocked { secrets } => {
            set_public_read_key(tx, &secrets.read_key).await?;
            obfuscate_secret_read_key(tx).await?;
            set_public_write_key(tx, secrets).await?;
            obfuscate_secret_write_key(tx).await?;

            Ok(LocalKeys {
                read: None,
                write: None,
            })
        }
        Access::WriteLocked {
            local_read_secret,
            local_write_secret,
            secrets,
        } => {
            let (local_read_key, local_read_salt) = secret_to_key_and_salt(local_read_secret);
            let (local_write_key, local_write_salt) = secret_to_key_and_salt(local_write_secret);

            remove_public_read_key(tx).await?;
            set_secret_read_key(
                tx,
                &secrets.id,
                &secrets.read_key,
                &local_read_key,
                &local_read_salt,
            )
            .await?;
            remove_public_write_key(tx).await?;
            set_secret_write_key(tx, secrets, &local_write_key, &local_write_salt).await?;

            Ok(LocalKeys {
                read: Some(local_read_key),
                write: Some(local_write_key),
            })
        }
        Access::WriteLockedReadUnlocked {
            local_write_secret,
            secrets,
        } => {
            let (local_write_key, local_write_salt) = secret_to_key_and_salt(local_write_secret);

            set_public_read_key(tx, &secrets.read_key).await?;
            obfuscate_secret_read_key(tx).await?;
            remove_public_write_key(tx).await?;
            set_secret_write_key(tx, secrets, &local_write_key, &local_write_salt).await?;

            Ok(LocalKeys {
                read: None,
                write: Some(local_write_key),
            })
        }
    }
}

pub(crate) struct LocalKeys<'a> {
    #[allow(unused)]
    pub read: Option<Cow<'a, cipher::SecretKey>>,
    pub write: Option<Cow<'a, cipher::SecretKey>>,
}

pub(crate) async fn get_access_secrets<'a>(
    tx: &mut db::WriteTransaction,
    local_secret: Option<&'a LocalSecret>,
) -> Result<(AccessSecrets, Option<Cow<'a, cipher::SecretKey>>), StoreError> {
    let id = get_repository_id(tx).await?;

    let local_write_key = match &local_secret {
        Some(local_secret) => Some(secret_to_key(tx, KeyType::Write, local_secret).await?),
        None => None,
    };

    match get_write_key(tx, local_write_key.as_deref(), &id).await {
        Ok(Some(write_keys)) => {
            let access = AccessSecrets::Write(WriteSecrets::from(write_keys));
            return Ok((access, local_write_key));
        }
        Ok(None) => (),
        Err(e) => return Err(e),
    }

    let local_read_key = match &local_secret {
        Some(local_secret) => Some(secret_to_key(tx, KeyType::Read, local_secret).await?),
        None => None,
    };

    // No match. Maybe there's the read key?
    match get_read_key(tx, local_read_key.as_deref(), &id).await {
        Ok(Some(read_key)) => {
            let access = AccessSecrets::Read { id, read_key };
            return Ok((access, local_write_key));
        }
        Ok(None) => (),
        Err(e) => return Err(e),
    }

    // No read key either, repository shall be open in blind mode.
    Ok((AccessSecrets::Blind { id }, None))
}

/// Returns Ok(None) when the key is there but isn't valid.
async fn get_write_key(
    conn: &mut db::Connection,
    local_key: Option<&cipher::SecretKey>,
    id: &RepositoryId,
) -> Result<Option<sign::Keypair>, StoreError> {
    // Try to interpret it first as the write keys.
    let write_keys: Option<sign::Keypair> = match get_blob(conn, WRITE_KEY, local_key).await? {
        Some(write_keys) => Some(write_keys),
        None => {
            // Let's be backward compatible.
            get_blob(conn, DEPRECATED_ACCESS_KEY, local_key).await?
        }
    };

    let Some(write_keys) = write_keys else {
        return Ok(None);
    };

    let derived_id = RepositoryId::from(write_keys.public_key());

    if &derived_id == id {
        Ok(Some(write_keys))
    } else {
        Ok(None)
    }
}

async fn get_read_key(
    conn: &mut db::Connection,
    local_key: Option<&cipher::SecretKey>,
    id: &RepositoryId,
) -> Result<Option<cipher::SecretKey>, StoreError> {
    let read_key: cipher::SecretKey = match get_blob(conn, READ_KEY, local_key).await {
        Ok(Some(read_key)) => read_key,
        Ok(None) => {
            // Let's be backward compatible.
            if let Some(key) = get_blob(conn, DEPRECATED_ACCESS_KEY, local_key).await? {
                key
            } else {
                return Ok(None);
            }
        }
        Err(error) => return Err(error),
    };

    if local_key.is_none() {
        return Ok(Some(read_key));
    }

    let key_validator_expected = read_key_validator(id);
    let key_validator_actual: Option<Hash> =
        get_secret_blob(conn, READ_KEY_VALIDATOR, &read_key).await?;

    if key_validator_actual == Some(key_validator_expected) {
        // Match - we have read access.
        Ok(Some(read_key))
    } else {
        Ok(None)
    }
}

// -------------------------------------------------------------------
// Storage quota
// -------------------------------------------------------------------
pub(crate) mod quota {
    use super::*;
    use crate::protocol::StorageSize;

    pub(crate) async fn get(conn: &mut db::Connection) -> Result<Option<StorageSize>, StoreError> {
        Ok(get_public(conn, QUOTA).await?.map(StorageSize::from_bytes))
    }

    pub(crate) async fn set(tx: &mut db::WriteTransaction, value: u64) -> Result<(), StoreError> {
        set_public(tx, QUOTA, value).await
    }

    pub(crate) async fn remove(tx: &mut db::WriteTransaction) -> Result<(), StoreError> {
        remove_public(tx, QUOTA).await
    }
}

// -------------------------------------------------------------------
// Storage block expiration
// -------------------------------------------------------------------
pub(crate) mod block_expiration {
    use super::*;

    pub(crate) async fn get(conn: &mut db::Connection) -> Result<Option<Duration>, StoreError> {
        Ok(get_public(conn, BLOCK_EXPIRATION)
            .await?
            .map(Duration::from_millis))
    }

    pub(crate) async fn set(
        tx: &mut db::WriteTransaction,
        value: Option<Duration>,
    ) -> Result<(), StoreError> {
        if let Some(duration) = value {
            set_public(
                tx,
                BLOCK_EXPIRATION,
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
            )
            .await
        } else {
            remove_public(tx, BLOCK_EXPIRATION).await
        }
    }
}

// -------------------------------------------------------------------
// Data version
// -------------------------------------------------------------------
pub(crate) mod data_version {
    use super::*;

    pub(crate) async fn get(conn: &mut db::Connection) -> Result<u64, StoreError> {
        Ok(get_public(conn, DATA_VERSION).await?.unwrap_or(0))
    }

    pub(crate) async fn set(tx: &mut db::WriteTransaction, value: u64) -> Result<(), StoreError> {
        set_public(tx, DATA_VERSION, value).await
    }
}

// -------------------------------------------------------------------
// Public values
// -------------------------------------------------------------------
async fn get_public_blob<T>(conn: &mut db::Connection, id: &[u8]) -> Result<Option<T>, StoreError>
where
    T: for<'a> TryFrom<&'a [u8]>,
{
    let row = sqlx::query("SELECT value FROM metadata_public WHERE name = ?")
        .bind(id)
        .fetch_optional(conn)
        .await?;

    if let Some(row) = row {
        let bytes: &[u8] = row.get(0);
        let bytes = bytes.try_into().map_err(|_| StoreError::MalformedData)?;
        Ok(Some(bytes))
    } else {
        Ok(None)
    }
}

async fn set_public_blob<T>(
    tx: &mut db::WriteTransaction,
    id: &[u8],
    blob: T,
) -> Result<(), StoreError>
where
    T: AsRef<[u8]>,
{
    sqlx::query("INSERT OR REPLACE INTO metadata_public(name, value) VALUES (?, ?)")
        .bind(id)
        .bind(blob.as_ref())
        .execute(tx)
        .await?;

    Ok(())
}

async fn get_public<T>(conn: &mut db::Connection, id: &[u8]) -> Result<Option<T>, StoreError>
where
    T: MetadataGet,
{
    let row = sqlx::query("SELECT value FROM metadata_public WHERE name = ?")
        .bind(id)
        .fetch_optional(conn)
        .await?;
    if let Some(row) = row {
        let value = T::get(&row).map_err(|_| StoreError::MalformedData)?;
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

async fn set_public<'a, T>(
    tx: &mut db::WriteTransaction,
    id: &'a [u8],
    value: T,
) -> Result<(), StoreError>
where
    T: MetadataSet<'a>,
{
    let query = sqlx::query("INSERT OR REPLACE INTO metadata_public(name, value) VALUES (?, ?)");
    let query = query.bind(id);
    let query = value.bind(query);
    query.execute(tx).await?;

    Ok(())
}

async fn remove_public(tx: &mut db::WriteTransaction, id: &[u8]) -> Result<(), StoreError> {
    sqlx::query("DELETE FROM metadata_public WHERE name = ?")
        .bind(id)
        .execute(tx)
        .await?;
    Ok(())
}

pub trait MetadataGet: detail::Get {}
pub trait MetadataSet<'a>: detail::Set<'a> {}

impl<T> MetadataGet for T where T: detail::Get {}
impl<'a, T> MetadataSet<'a> for T where T: detail::Set<'a> {}

// Use the sealed trait pattern to avoid exposing implementation details outside of this crate.
mod detail {
    use crate::db;
    use sqlx::{
        sqlite::{SqliteArguments, SqliteRow},
        Row, Sqlite,
    };

    type Query<'q> = sqlx::query::Query<'q, Sqlite, SqliteArguments<'q>>;

    pub trait Get: Sized {
        fn get(row: &SqliteRow) -> Result<Self, sqlx::Error>;
    }

    pub trait Set<'a>
    where
        Self: 'a,
    {
        fn bind(self, query: Query<'a>) -> Query<'a>;
    }

    impl Get for bool {
        fn get(row: &SqliteRow) -> Result<Self, sqlx::Error> {
            row.try_get(0)
        }
    }

    impl<'a> Set<'a> for bool {
        fn bind(self, query: Query<'a>) -> Query<'a> {
            query.bind(self)
        }
    }

    impl Get for u64 {
        fn get(row: &SqliteRow) -> Result<Self, sqlx::Error> {
            row.try_get(0).map(db::decode_u64)
        }
    }

    impl<'a> Set<'a> for u64 {
        fn bind(self, query: Query<'a>) -> Query<'a> {
            query.bind(db::encode_u64(self))
        }
    }

    impl Get for String {
        fn get(row: &SqliteRow) -> Result<Self, sqlx::Error> {
            row.try_get(0)
        }
    }

    impl<'a> Set<'a> for String {
        fn bind(self, query: Query<'a>) -> Query<'a> {
            query.bind(self)
        }
    }

    impl<'a> Set<'a> for &'a str {
        fn bind(self, query: Query<'a>) -> Query<'a> {
            query.bind(self)
        }
    }
}

// -------------------------------------------------------------------
// Secret values
// -------------------------------------------------------------------
async fn get_secret_blob<T>(
    conn: &mut db::Connection,
    id: &[u8],
    local_key: &cipher::SecretKey,
) -> Result<Option<T>, StoreError>
where
    for<'a> T: TryFrom<&'a [u8]>,
{
    let row = sqlx::query("SELECT nonce, value FROM metadata_secret WHERE name = ?")
        .bind(id)
        .fetch_optional(conn)
        .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let nonce: &[u8] = row.get(0);
    let nonce = Nonce::try_from(nonce).map_err(|_| StoreError::MalformedData)?;

    let mut buffer: Vec<_> = row.get(1);

    local_key.decrypt_no_aead(&nonce, &mut buffer);

    let secret = T::try_from(&buffer).map_err(|_| StoreError::MalformedData)?;
    buffer.zeroize();

    Ok(Some(secret))
}

async fn set_secret_blob<T>(
    tx: &mut db::WriteTransaction,
    id: &[u8],
    blob: T,
    local_key: &cipher::SecretKey,
) -> Result<(), StoreError>
where
    T: AsRef<[u8]>,
{
    let nonce = make_nonce();

    let mut cypher = blob.as_ref().to_vec();
    local_key.encrypt_no_aead(&nonce, &mut cypher);

    sqlx::query(
        "INSERT OR REPLACE INTO metadata_secret(name, nonce, value)
            VALUES (?, ?, ?)",
    )
    .bind(id)
    .bind(&nonce[..])
    .bind(&cypher)
    .execute(tx)
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
async fn get_blob<T>(
    conn: &mut db::Connection,
    id: &[u8],
    local_key: Option<&cipher::SecretKey>,
) -> Result<Option<T>, StoreError>
where
    for<'a> T: TryFrom<&'a [u8]>,
{
    match local_key {
        Some(local_key) => get_secret_blob(conn, id, local_key).await,
        None => get_public_blob(conn, id).await,
    }
}

async fn set_blob<T>(
    tx: &mut db::WriteTransaction,
    id: &[u8],
    blob: T,
    local_key: Option<&cipher::SecretKey>,
) -> Result<(), StoreError>
where
    T: AsRef<[u8]>,
{
    match local_key {
        Some(local_key) => set_secret_blob(tx, id, blob, local_key).await,
        None => set_public_blob(tx, id, blob).await,
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
        let mut tx = pool.begin_write().await.unwrap();

        set_public_blob(&mut tx, b"hello", b"world").await.unwrap();

        let v: [u8; 5] = get_public_blob(&mut tx, b"hello").await.unwrap().unwrap();

        assert_eq!(b"world", &v);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_cyphertext() {
        let (_base_dir, pool) = setup().await;
        let mut tx = pool.begin_write().await.unwrap();

        let key = cipher::SecretKey::random();

        set_secret_blob(&mut tx, b"hello", b"world", &key)
            .await
            .unwrap();

        let v: [u8; 5] = get_secret_blob(&mut tx, b"hello", &key)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(b"world", &v);
    }

    // Using a bad key should not decrypt properly, but also should not cause an error. This is to
    // let user claim plausible deniability in not knowing the real secret key/password.
    #[tokio::test(flavor = "multi_thread")]
    async fn bad_key_is_not_error() {
        let (_base_dir, pool) = setup().await;
        let mut tx = pool.begin_write().await.unwrap();

        let good_key = cipher::SecretKey::random();
        let bad_key = cipher::SecretKey::random();

        set_secret_blob(&mut tx, b"hello", b"world", &good_key)
            .await
            .unwrap();

        let v: [u8; 5] = get_secret_blob(&mut tx, b"hello", &bad_key)
            .await
            .unwrap()
            .unwrap();

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
                local_secret: SetLocalSecret::random(),
                read_key: cipher::SecretKey::random(),
            },
            Access::WriteUnlocked {
                secrets: WriteSecrets::random(),
            },
            Access::WriteLocked {
                local_read_secret: SetLocalSecret::random(),
                local_write_secret: SetLocalSecret::random(),
                secrets: WriteSecrets::random(),
            },
            Access::WriteLockedReadUnlocked {
                local_write_secret: SetLocalSecret::random(),
                secrets: WriteSecrets::random(),
            },
        ];

        for access in accesses {
            let (_base_dir, pool) = db::create_temp().await.unwrap();

            let mut tx = pool.begin_write().await.unwrap();
            let local_keys = initialize_access_secrets(&mut tx, &access).await.unwrap();
            tx.commit().await.unwrap();

            let local_key = local_keys
                .write
                .as_deref()
                .or(local_keys.read.as_deref())
                .cloned();

            let mut tx = pool.begin_write().await.unwrap();

            let local_secret = local_key.clone().map(LocalSecret::SecretKey);

            let access_secrets = get_access_secrets(&mut tx, local_secret.as_ref())
                .await
                .unwrap();

            assert_eq!(
                (access.secrets(), local_key.as_ref()),
                (access_secrets.0, access_secrets.1.as_deref())
            );
        }
    }
}
