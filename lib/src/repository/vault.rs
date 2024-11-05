//! Repository state and operations that don't require read or write access.

#[cfg(test)]
mod tests;

use super::{quota, Metadata, RepositoryMonitor};
use crate::{
    block_tracker::BlockTracker,
    db,
    debug::DebugPrinter,
    error::Result,
    event::EventSender,
    protocol::{RepositoryId, StorageSize},
    store::Store,
};
use sqlx::Row;
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};
use tracing::Instrument;

#[derive(Clone)]
pub(crate) struct Vault {
    repository_id: RepositoryId,
    store: Store,
    pub event_tx: EventSender,
    pub block_tracker: BlockTracker,
    pub monitor: Arc<RepositoryMonitor>,
}

impl Vault {
    pub fn new(
        repository_id: RepositoryId,
        event_tx: EventSender,
        pool: db::Pool,
        monitor: RepositoryMonitor,
    ) -> Self {
        let store = Store::new(pool);

        Self {
            repository_id,
            store,
            event_tx,
            block_tracker: BlockTracker::new(),
            monitor: Arc::new(monitor),
        }
    }

    pub fn repository_id(&self) -> &RepositoryId {
        &self.repository_id
    }

    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    pub fn metadata(&self) -> Metadata {
        Metadata::new(self.store().db().clone())
    }

    /// Total size of the stored data
    pub async fn size(&self) -> Result<StorageSize> {
        let mut conn = self.store().db().acquire().await?;

        // Note: for simplicity, we are currently counting only blocks (content + id + nonce)
        let count = db::decode_u64(
            sqlx::query("SELECT COUNT(*) FROM blocks")
                .fetch_one(&mut *conn)
                .await?
                .get(0),
        );

        Ok(StorageSize::from_blocks(count))
    }

    pub async fn set_quota(&self, quota: Option<StorageSize>) -> Result<()> {
        let mut tx = self.store().db().begin_write().await?;

        if let Some(quota) = quota {
            quota::set(&mut tx, quota.to_bytes()).await?
        } else {
            quota::remove(&mut tx).await?
        }

        tx.commit().await?;

        Ok(())
    }

    pub async fn quota(&self) -> Result<Option<StorageSize>> {
        let mut conn = self.store().db().acquire().await?;
        Ok(quota::get(&mut conn).await?)
    }

    pub async fn set_block_expiration(&self, duration: Option<Duration>) -> Result<()> {
        Ok(self
            .store
            .set_block_expiration(duration, self.block_tracker.clone())
            .instrument(self.monitor.span().clone())
            .await?)
    }

    pub async fn block_expiration(&self) -> Option<Duration> {
        self.store.block_expiration().await
    }

    pub async fn last_block_expiration_time(&self) -> Option<SystemTime> {
        self.store.last_block_expiration_time().await
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        self.store().debug_print_root_node(print).await
    }
}
