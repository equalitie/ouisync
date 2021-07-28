use crate::{
    db,
    error::{Error, Result},
    replica_id::ReplicaId,
};

use sqlx::Row;
use std::convert::TryFrom;

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

pub async fn get_id(pool: &db::Pool) -> Result<Option<ReplicaId>> {
    let mut conn = pool.acquire().await?;

    let row = sqlx::query("SELECT replica_id FROM this_replica_id WHERE row_id=0;")
        .fetch_optional(&mut conn)
        .await?;

    match row {
        Some(row) => {
            let blob: &[u8] = row.get(0);
            Ok(Some(ReplicaId::try_from(blob)?))
        }
        None => Ok(None),
    }
}

pub async fn set_id(pool: &db::Pool, replica_id: &ReplicaId) -> Result<(), Error> {
    let mut conn = pool.acquire().await?;

    sqlx::query(
        "INSERT INTO this_replica_id(row_id, replica_id)
             VALUES (0, ?) ON CONFLICT(row_id) DO UPDATE SET replica_id=?;",
    )
    .bind(replica_id)
    .bind(replica_id)
    .execute(&mut conn)
    .await?;

    Ok(())
}

pub async fn get_or_create_id(pool: &db::Pool) -> Result<ReplicaId> {
    match get_id(pool).await? {
        Some(id) => Ok(id),
        None => {
            let id = rand::random();
            set_id(pool, &id).await?;
            Ok(id)
        }
    }
}
