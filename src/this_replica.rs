use crate::{
    db,
    error::{Error, Result},
    replica_id::ReplicaId,
};
use sqlx::Row;

/// Initializes the table that holds a single entry: the ReplicaId of this replica
pub async fn init(pool: &db::Pool) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS this_replica_id (
             -- row_id is always zero and is used to rewrite the row if there is one already
             -- in the table.
             row_id     INTEGER PRIMARY KEY,
             replica_id BLOB NOT NULL
         );",
    )
    .execute(pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}

pub async fn get_id(tx: &mut db::Transaction<'_>) -> Result<Option<ReplicaId>> {
    let id = sqlx::query("SELECT replica_id FROM this_replica_id WHERE row_id=0")
        .fetch_optional(tx)
        .await?
        .map(|row| row.get(0));

    Ok(id)
}

pub async fn set_id(tx: &mut db::Transaction<'_>, replica_id: &ReplicaId) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO this_replica_id(row_id, replica_id)
             VALUES (0, ?) ON CONFLICT(row_id) DO UPDATE SET replica_id=?",
    )
    .bind(replica_id)
    .bind(replica_id)
    .execute(tx)
    .await?;

    Ok(())
}

pub async fn get_or_create_id(db: impl db::Acquire<'_>) -> Result<ReplicaId> {
    // Use db transaction to make the operations atomic.
    let mut tx = db.begin().await?;

    let id = match get_id(&mut tx).await? {
        Some(id) => id,
        None => {
            let id = rand::random();
            set_id(&mut tx, &id).await?;
            id
        }
    };

    tx.commit().await?;

    Ok(id)
}
