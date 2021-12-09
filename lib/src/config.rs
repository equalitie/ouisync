use crate::{db, error::Result, this_writer};

/// Opens the config database.
pub async fn open_db(store: &db::Store) -> Result<db::Pool> {
    let pool = db::open(store).await?;
    this_writer::init(&pool).await?;

    Ok(pool)
}
