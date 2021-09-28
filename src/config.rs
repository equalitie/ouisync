use crate::{db, error::Result, repository, this_replica};

/// Opens the config database.
pub async fn open_db(store: db::Store) -> Result<db::Pool> {
    let pool = db::open(store).await?;

    this_replica::init(&pool).await?;
    repository::init_manager(&pool).await?;

    Ok(pool)
}
