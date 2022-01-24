use crate::{
    db,
    error::{Error, Result},
};
use rand::{rngs::OsRng, Rng};
use sqlx::{Acquire, Row};

define_byte_array_wrapper! {
    /// DeviceId uniquely identifies machines on which this software is running. Its only purpose is
    /// to ensure that one WriterId (which currently equates to sing::PublicKey) will never create two
    /// or more concurrent snapshots as that would break the whole repository.  This is achieved by
    /// ensuring that there is always only a single DeviceId associated with a single WriterId.
    ///
    /// This means that whenever the database is copied/moved from one device to another, the database
    /// containing the DeviceId MUST either not be migrated with it, OR ensure that it'll never be
    /// used from its original place.
    ///
    /// ReplicaIds are private and thus not shared over the network.
    pub struct DeviceId([u8; 32]);
}

derive_rand_for_wrapper!(DeviceId);
derive_sqlx_traits_for_byte_array_wrapper!(DeviceId);

/// Initializes the table that holds a single entry: the DeviceId of this replica
pub(crate) async fn init(pool: &db::Pool) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS device_id (
             -- row_id is always zero and is used to rewrite the row if there is one already
             -- in the table.
             row_id INTEGER PRIMARY KEY,
             value  BLOB NOT NULL
         );",
    )
    .execute(pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}

pub async fn get_or_create(conn: &mut db::Connection) -> Result<DeviceId> {
    // Use db transaction to make the operations atomic.
    let mut tx = conn.begin().await?;

    let id = match get(&mut tx).await? {
        Some(id) => id,
        None => {
            let id = OsRng.gen();
            set(&mut tx, &id).await?;
            id
        }
    };

    tx.commit().await?;

    Ok(id)
}

async fn get(conn: &mut db::Connection) -> Result<Option<DeviceId>> {
    let id = sqlx::query("SELECT value FROM device_id WHERE row_id=0")
        .fetch_optional(conn)
        .await?
        .map(|row| row.get(0));

    Ok(id)
}

async fn set(conn: &mut db::Connection, writer_id: &DeviceId) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO device_id(row_id, value)
             VALUES (0, ?) ON CONFLICT(row_id) DO UPDATE SET value=?",
    )
    .bind(writer_id)
    .bind(writer_id)
    .execute(conn)
    .await?;

    Ok(())
}
