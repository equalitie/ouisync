use crate::{db, error::Result, this_replica};

/// Opens the config database.
pub async fn open_db(store: &db::Store) -> Result<db::Pool> {
    let pool = db::open(store).await?;
    this_replica::init(&pool).await?;

    Ok(pool)
}
