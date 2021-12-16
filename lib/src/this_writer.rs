use crate::{
    crypto::sign::{Keypair, PublicKey},
    db,
    error::{Error, Result},
};
use sqlx::Row;

/// Initializes the table that holds a single entry: the PublicKey of this replica
pub(crate) async fn init(pool: &db::Pool) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS this_writer_id (
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

async fn get_id(tx: &mut db::Transaction<'_>) -> Result<Option<PublicKey>> {
    let id = sqlx::query("SELECT writer_id FROM this_writer_id WHERE row_id=0")
        .fetch_optional(tx)
        .await?
        .map(|row| row.get(0));

    Ok(id)
}

async fn set_id(tx: &mut db::Transaction<'_>, writer_id: &PublicKey) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO this_writer_id(row_id, writer_id)
             VALUES (0, ?) ON CONFLICT(row_id) DO UPDATE SET writer_id=?",
    )
    .bind(writer_id)
    .bind(writer_id)
    .execute(tx)
    .await?;

    Ok(())
}

pub async fn get_or_create_id(db: impl db::Acquire<'_>) -> Result<PublicKey> {
    // Use db transaction to make the operations atomic.
    let mut tx = db.begin().await?;

    let id = match get_id(&mut tx).await? {
        Some(id) => id,
        None => {
            let keypair = Keypair::generate();
            let id = keypair.public;
            set_id(&mut tx, &id).await?;
            id
        }
    };

    tx.commit().await?;

    Ok(id)
}
