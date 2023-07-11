use super::error::Error;
use crate::{block::BlockId, db};

pub(super) async fn remove(tx: &mut db::WriteTransaction, id: &BlockId) -> Result<(), Error> {
    sqlx::query("DELETE FROM blocks WHERE id = ?")
        .bind(id)
        .execute(tx)
        .await?;

    Ok(())
}
