use crate::{
    access_control::{Access, AccessSecrets, LocalSecret, WriteSecrets},
    crypto::{
        cipher::{self, Nonce},
        sign, Hash, Password, PasswordSalt,
    },
    db::{self, DatabaseId},
    device_id::DeviceId,
    error::{Error, Result},
    repository::RepositoryId,
};
use rand::{rngs::OsRng, Rng};
use sqlx::Row;
use std::{borrow::Cow, fmt, time::Duration};
use tracing::instrument;
use zeroize::Zeroize;

// Metadata keys
const REPOSITORY_ID: &[u8] = b"repository_id";
const PASSWORD_SALT: &[u8] = b"password_salt";
const WRITER_ID: &[u8] = b"writer_id";
const READ_KEY: &[u8] = b"read_key";
const WRITE_KEY: &[u8] = b"write_key";
const DATABASE_ID: &[u8] = b"database_id";

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

const QUOTA: &[u8] = b"quota";
const BLOCK_EXPIRATION: &[u8] = b"block_expiration";

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
    pub async fn get<T>(&self, name: &str) -> Result<T>
    where
        T: MetadataGet + fmt::Debug,
    {
        let mut conn = self.db.acquire().await.map_err(|error| {
            tracing::error!(?error);
            error
        })?;
        let value = get_public(&mut conn, name.as_bytes())
            .await
            .map_err(|error| {
                match error {
                    Error::EntryNotFound => (),
                    _ => tracing::error!(?error),
                }

                error
            })?;

        Ok(value)
    }

    #[instrument(skip(self), err(Debug))]
    pub async fn set<'a, T>(&self, name: &'a str, value: T) -> Result<()>
    where
        T: MetadataSet<'a> + fmt::Debug,
    {
        let mut tx = self.db.begin_write().await?;
        set_public(&mut tx, name.as_bytes(), value).await?;
        tx.commit().await?;

        Ok(())
    }

    #[instrument(skip(self), err(Debug))]
    pub async fn remove(&self, name: &str) -> Result<()> {
        let mut tx = self.db.begin_write().await?;
        remove_public(&mut tx, name.as_bytes()).await?;
        tx.commit().await?;

        Ok(())
    }
}

// -------------------------------------------------------------------
// Password
// -------------------------------------------------------------------
pub(crate) async fn password_to_key(
    tx: &mut db::WriteTransaction,
    password: &Password,
) -> Result<cipher::SecretKey> {
    let salt = get_or_generate_password_salt(tx).await?;
    Ok(cipher::SecretKey::derive_from_password(
        password.as_ref(),
        &salt,
    ))
}

pub(crate) async fn secret_to_key<'a>(
    tx: &mut db::WriteTransaction,
    secret: &'a LocalSecret,
) -> Result<Cow<'a, cipher::SecretKey>> {
    match secret {
        LocalSecret::Password(password) => password_to_key(tx, password).await.map(Cow::Owned),
        LocalSecret::SecretKey(key) => Ok(Cow::Borrowed(key)),
    }
}

async fn get_or_generate_password_salt(tx: &mut db::WriteTransaction) -> Result<PasswordSalt> {
    let salt = match get_public_blob(tx, PASSWORD_SALT).await {
        Ok(salt) => salt,
        Err(Error::EntryNotFound) => {
            let salt: PasswordSalt = OsRng.gen();
            set_public_blob(tx, PASSWORD_SALT, &salt).await?;
            salt
        }
        Err(error) => return Err(error),
    };

    Ok(salt)
}

// -------------------------------------------------------------------
// Database ID
// -------------------------------------------------------------------
pub(crate) async fn get_or_generate_database_id(db: &db::Pool) -> Result<DatabaseId> {
    let mut tx = db.begin_write().await?;
    let database_id = match get_public_blob(&mut tx, DATABASE_ID).await {
        Ok(database_id) => database_id,
        Err(Error::EntryNotFound) => {
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
) -> Result<sign::PublicKey> {
    get_blob(conn, WRITER_ID, local_key).await
}

pub(crate) async fn set_writer_id(
    tx: &mut db::WriteTransaction,
    writer_id: &sign::PublicKey,
    local_key: Option<&cipher::SecretKey>,
) -> Result<()> {
    set_blob(tx, WRITER_ID, writer_id, local_key).await?;
    Ok(())
}

// -------------------------------------------------------------------
// Device id
// -------------------------------------------------------------------

// Checks whether the stored device id is the same as the specified one.
pub(crate) async fn check_device_id(
    conn: &mut db::Connection,
    device_id: &DeviceId,
) -> Result<bool> {
    let old_device_id: DeviceId = get_public_blob(conn, DEVICE_ID).await?;
    Ok(old_device_id == *device_id)
}

pub(crate) async fn set_device_id(
    tx: &mut db::WriteTransaction,
    device_id: &DeviceId,
) -> Result<()> {
    set_public_blob(tx, DEVICE_ID, device_id).await?;
    Ok(())
}

// -------------------------------------------------------------------
// Access secrets
// -------------------------------------------------------------------
async fn set_public_read_key(
    tx: &mut db::WriteTransaction,
    read_key: &cipher::SecretKey,
) -> Result<()> {
    set_public_blob(tx, READ_KEY, read_key).await
}

async fn set_secret_read_key(
    tx: &mut db::WriteTransaction,
    id: &RepositoryId,
    read_key: &cipher::SecretKey,
    local_key: &cipher::SecretKey,
) -> Result<()> {
    set_secret_blob(tx, READ_KEY, read_key, local_key).await?;
    set_secret_blob(tx, READ_KEY_VALIDATOR, read_key_validator(id), read_key).await
}

pub(crate) async fn set_read_key(
    tx: &mut db::WriteTransaction,
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

async fn remove_public_read_key(tx: &mut db::WriteTransaction) -> Result<()> {
    remove_public(tx, READ_KEY).await
}

async fn remove_secret_read_key(tx: &mut db::WriteTransaction) -> Result<()> {
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

    Ok(())
}

pub(crate) async fn remove_read_key(tx: &mut db::WriteTransaction) -> Result<()> {
    remove_public_read_key(tx).await?;
    remove_secret_read_key(tx).await
}

// ------------------------------

async fn set_public_write_key(tx: &mut db::WriteTransaction, secrets: &WriteSecrets) -> Result<()> {
    set_public_blob(tx, WRITE_KEY, secrets.write_keys.to_bytes()).await
}

async fn set_secret_write_key(
    tx: &mut db::WriteTransaction,
    secrets: &WriteSecrets,
    local_key: &cipher::SecretKey,
) -> Result<()> {
    set_secret_blob(tx, WRITE_KEY, secrets.write_keys.to_bytes(), local_key).await
}

pub(crate) async fn set_write_key(
    tx: &mut db::WriteTransaction,
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

async fn remove_public_write_key(tx: &mut db::WriteTransaction) -> Result<()> {
    remove_public(tx, WRITE_KEY).await
}

async fn remove_secret_write_key(tx: &mut db::WriteTransaction) -> Result<()> {
    let dummy_local_key = cipher::SecretKey::random();
    let dummy_write_key = sign::Keypair::random().to_bytes();
    set_secret_blob(tx, WRITE_KEY, &dummy_write_key, &dummy_local_key).await
}

pub(crate) async fn remove_write_key(tx: &mut db::WriteTransaction) -> Result<()> {
    remove_public_write_key(tx).await?;
    remove_secret_write_key(tx).await
}

// ------------------------------

pub(crate) async fn requires_local_password_for_reading(conn: &mut db::Connection) -> Result<bool> {
    match get_public_blob::<cipher::SecretKey>(conn, READ_KEY).await {
        Ok(_) => return Ok(false),
        Err(Error::EntryNotFound) => (),
        Err(err) => return Err(err),
    }

    match get_public_blob::<sign::Keypair>(conn, WRITE_KEY).await {
        Ok(_) => Ok(false),
        Err(Error::EntryNotFound) => Ok(true),
        Err(err) => Err(err),
    }
}

pub(crate) async fn requires_local_password_for_writing(conn: &mut db::Connection) -> Result<bool> {
    match get_public_blob::<sign::Keypair>(conn, WRITE_KEY).await {
        Ok(_) => Ok(false),
        Err(Error::EntryNotFound) => Ok(true),
        Err(err) => Err(err),
    }
}

pub(crate) async fn initialize_access_secrets<'a>(
    tx: &mut db::WriteTransaction,
    access: &'a Access,
) -> Result<LocalKeys<'a>> {
    set_public_blob(tx, REPOSITORY_ID, access.id()).await?;
    set_access(tx, access).await
}

pub(crate) async fn set_access<'a>(
    tx: &mut db::WriteTransaction,
    access: &'a Access,
) -> Result<LocalKeys<'a>> {
    match access {
        Access::Blind { .. } => {
            remove_public_read_key(tx).await?;
            remove_secret_read_key(tx).await?;
            remove_public_write_key(tx).await?;
            remove_secret_write_key(tx).await?;

            Ok(LocalKeys {
                read: None,
                write: None,
            })
        }
        Access::ReadUnlocked { id: _, read_key } => {
            set_public_read_key(tx, read_key).await?;
            remove_secret_read_key(tx).await?;
            remove_public_write_key(tx).await?;
            remove_secret_write_key(tx).await?;

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
            let local_key = secret_to_key(tx, local_secret).await?;

            remove_public_read_key(tx).await?;
            set_secret_read_key(tx, id, read_key, &local_key).await?;
            remove_public_write_key(tx).await?;
            remove_secret_write_key(tx).await?;

            Ok(LocalKeys {
                read: Some(local_key),
                write: None,
            })
        }
        Access::WriteUnlocked { secrets } => {
            set_public_read_key(tx, &secrets.read_key).await?;
            remove_secret_read_key(tx).await?;
            set_public_write_key(tx, secrets).await?;
            remove_secret_write_key(tx).await?;

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
            let local_read_key = secret_to_key(tx, local_read_secret).await?;
            let local_write_key = secret_to_key(tx, local_write_secret).await?;

            remove_public_read_key(tx).await?;
            set_secret_read_key(tx, &secrets.id, &secrets.read_key, &local_read_key).await?;
            remove_public_write_key(tx).await?;
            set_secret_write_key(tx, secrets, &local_write_key).await?;

            Ok(LocalKeys {
                read: Some(local_read_key),
                write: Some(local_write_key),
            })
        }
        Access::WriteLockedReadUnlocked {
            local_write_secret,
            secrets,
        } => {
            let local_write_key = secret_to_key(tx, local_write_secret).await?;

            set_public_read_key(tx, &secrets.read_key).await?;
            remove_secret_read_key(tx).await?;
            remove_public_write_key(tx).await?;
            set_secret_write_key(tx, secrets, &local_write_key).await?;

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

pub(crate) async fn get_access_secrets(
    conn: &mut db::Connection,
    local_key: Option<&cipher::SecretKey>,
) -> Result<AccessSecrets> {
    let id = get_public_blob(conn, REPOSITORY_ID).await?;

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
    // Try to interpret it first as the write keys.
    let write_keys: sign::Keypair = match get_blob(conn, WRITE_KEY, local_key).await {
        Ok(write_keys) => write_keys,
        Err(Error::EntryNotFound) => {
            // Let's be backward compatible.
            get_blob(conn, DEPRECATED_ACCESS_KEY, local_key).await?
        }
        Err(error) => return Err(error),
    };

    let derived_id = RepositoryId::from(write_keys.public_key());

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
    let read_key: cipher::SecretKey = match get_blob(conn, READ_KEY, local_key).await {
        Ok(read_key) => read_key,
        Err(Error::EntryNotFound) => {
            // Let's be backward compatible.
            get_blob(conn, DEPRECATED_ACCESS_KEY, local_key).await?
        }
        Err(error) => return Err(error),
    };

    if local_key.is_none() {
        return Ok(read_key);
    }

    let key_validator_expected = read_key_validator(id);
    let key_validator_actual: Hash = get_secret_blob(conn, READ_KEY_VALIDATOR, &read_key).await?;

    if key_validator_actual == key_validator_expected {
        // Match - we have read access.
        Ok(read_key)
    } else {
        Err(Error::EntryNotFound)
    }
}

// -------------------------------------------------------------------
// Storage quota
// -------------------------------------------------------------------
pub(crate) mod quota {
    use super::*;

    pub(crate) async fn get(conn: &mut db::Connection) -> Result<u64> {
        get_public(conn, QUOTA).await
    }

    pub(crate) async fn set(tx: &mut db::WriteTransaction, value: u64) -> Result<()> {
        set_public(tx, QUOTA, value).await
    }

    pub(crate) async fn remove(tx: &mut db::WriteTransaction) -> Result<()> {
        remove_public(tx, QUOTA).await
    }
}

// -------------------------------------------------------------------
// Storage block expiration
// -------------------------------------------------------------------
pub(crate) mod block_expiration {
    use super::*;

    pub(crate) async fn get(conn: &mut db::Connection) -> Result<Option<Duration>> {
        match get_public(conn, BLOCK_EXPIRATION).await {
            Ok(duration_millis) => Ok(Some(Duration::from_millis(duration_millis))),
            Err(Error::EntryNotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn set(tx: &mut db::WriteTransaction, value: Option<Duration>) -> Result<()> {
        if let Some(duration) = value {
            set_public(
                tx,
                BLOCK_EXPIRATION,
                u64::try_from(duration.as_millis()).map_err(|_| Error::InvalidArgument)?,
            )
            .await
        } else {
            remove_public(tx, BLOCK_EXPIRATION).await
        }
    }
}

// -------------------------------------------------------------------
// Public values
// -------------------------------------------------------------------
async fn get_public_blob<T>(conn: &mut db::Connection, id: &[u8]) -> Result<T>
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

async fn set_public_blob<T>(tx: &mut db::WriteTransaction, id: &[u8], blob: T) -> Result<()>
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

async fn get_public<T>(conn: &mut db::Connection, id: &[u8]) -> Result<T>
where
    T: MetadataGet,
{
    let row = sqlx::query("SELECT value FROM metadata_public WHERE name = ?")
        .bind(id)
        .fetch_optional(conn)
        .await?;
    let row = row.ok_or(Error::EntryNotFound)?;
    T::get(&row).map_err(|_| Error::MalformedData)
}

async fn set_public<'a, T>(tx: &mut db::WriteTransaction, id: &'a [u8], value: T) -> Result<()>
where
    T: MetadataSet<'a>,
{
    let query = sqlx::query("INSERT OR REPLACE INTO metadata_public(name, value) VALUES (?, ?)");
    let query = query.bind(id);
    let query = value.bind(query);
    query.execute(tx).await?;

    Ok(())
}

async fn remove_public(tx: &mut db::WriteTransaction, id: &[u8]) -> Result<()> {
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

async fn set_secret_blob<T>(
    tx: &mut db::WriteTransaction,
    id: &[u8],
    blob: T,
    local_key: &cipher::SecretKey,
) -> Result<()>
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
) -> Result<T>
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
) -> Result<()>
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

        let v: [u8; 5] = get_public_blob(&mut tx, b"hello").await.unwrap();

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

        let v: [u8; 5] = get_secret_blob(&mut tx, b"hello", &key).await.unwrap();

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

        let v: [u8; 5] = get_secret_blob(&mut tx, b"hello", &bad_key).await.unwrap();

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
                local_secret: LocalSecret::SecretKey(cipher::SecretKey::random()),
                read_key: cipher::SecretKey::random(),
            },
            Access::WriteUnlocked {
                secrets: WriteSecrets::random(),
            },
            Access::WriteLocked {
                local_read_secret: LocalSecret::SecretKey(cipher::SecretKey::random()),
                local_write_secret: LocalSecret::SecretKey(cipher::SecretKey::random()),
                secrets: WriteSecrets::random(),
            },
            Access::WriteLockedReadUnlocked {
                local_write_secret: LocalSecret::SecretKey(cipher::SecretKey::random()),
                secrets: WriteSecrets::random(),
            },
        ];

        for access in accesses {
            let (_base_dir, pool) = db::create_temp().await.unwrap();

            let mut tx = pool.begin_write().await.unwrap();
            let local_keys = initialize_access_secrets(&mut tx, &access).await.unwrap();
            tx.commit().await.unwrap();

            let local_key = local_keys.write.as_deref().or(local_keys.read.as_deref());

            let mut conn = pool.acquire().await.unwrap();

            let access_secrets = get_access_secrets(&mut conn, local_key).await.unwrap();

            assert_eq!(access.secrets(), access_secrets);
        }
    }
}
