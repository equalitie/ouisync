use crate::{db, error::Result, replica_id};

/// Opens the config database.
pub async fn open_db(store: &db::Store) -> Result<db::Pool> {
    let pool = db::open(store).await?;
    replica_id::init_this_replica(&pool).await?;

    Ok(pool)
}
