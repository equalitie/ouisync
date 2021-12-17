use crate::{
    db,
    error::{Error, Result},
};
use rand::{rngs::OsRng, Rng};
use sqlx::Row;

define_byte_array_wrapper! {
    /// ReplicaId uniquely identifies machines on which this software is running. Its only purpose is
    /// to ensure that one WriterId (which currently equates to sing::PublicKey) will never create two
    /// or more concurrent snapshots as that would break the whole repository.  This is achieved by
    /// ensuring that there is always only a single ReplicaId associated with a single WriterId.
    ///
    /// This means that whenever the database is copied/moved from one device to another, the database
    /// containing the ReplicaId MUST either not be migrated with it, OR ensure that it'll never be
    /// used from its original place.
    ///
    /// ReplicaIds are private and thus not shared over the network.
    pub struct ReplicaId([u8; 32]);
}

derive_rand_for_wrapper!(ReplicaId);
derive_sqlx_traits_for_byte_array_wrapper!(ReplicaId);

/// Initializes the table that holds a single entry: the ReplicaId of this replica
pub(crate) async fn init_this_replica(pool: &db::Pool) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS this_replica_id (
             -- row_id is always zero and is used to rewrite the row if there is one already
             -- in the table.
             row_id     INTEGER PRIMARY KEY,
             writer_id BLOB NOT NULL
         );",
    )
    .execute(pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}

pub async fn get_or_create_this_replica_id(db: impl db::Acquire<'_>) -> Result<ReplicaId> {
    // Use db transaction to make the operations atomic.
    let mut tx = db.begin().await?;

    let id = match get_id(&mut tx).await? {
        Some(id) => id,
        None => {
            let id = OsRng.gen();
            set_id(&mut tx, &id).await?;
            id
        }
    };

    tx.commit().await?;

    Ok(id)
}

async fn get_id(tx: &mut db::Transaction<'_>) -> Result<Option<ReplicaId>> {
    let id = sqlx::query("SELECT writer_id FROM this_replica_id WHERE row_id=0")
        .fetch_optional(tx)
        .await?
        .map(|row| row.get(0));

    Ok(id)
}

async fn set_id(tx: &mut db::Transaction<'_>, writer_id: &ReplicaId) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO this_replica_id(row_id, writer_id)
             VALUES (0, ?) ON CONFLICT(row_id) DO UPDATE SET writer_id=?",
    )
    .bind(writer_id)
    .bind(writer_id)
    .execute(tx)
    .await?;

    Ok(())
}
