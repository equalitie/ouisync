use crate::{db, device_id, error::Result};

/// Opens the config database.
pub async fn open_db(store: &db::Store) -> Result<db::Pool> {
    let pool = db::open_or_create(store).await?;
    device_id::init(&pool).await?;

    Ok(pool)
}
