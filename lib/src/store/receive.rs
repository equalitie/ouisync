//! Receiving data from other replicas and writing them into the store.

use super::{
    error::Error,
    quota::{self, QuotaError},
    root_node,
};
use crate::{
    crypto::Hash,
    db,
    future::try_collect_into,
    index::{update_summaries, NodeState, ReceiveStatus, UpdateSummaryReason},
    storage_size::StorageSize,
};

pub(super) async fn finalize(
    tx: &mut db::WriteTransaction,
    hash: Hash,
    quota: Option<StorageSize>,
) -> Result<ReceiveStatus, Error> {
    // TODO: Don't hold write transaction through this whole function. Use it only for
    // `update_summaries` then commit it, then do the quota check with a read-only transaction
    // and then grab another write transaction to do the `approve` / `reject`.
    // CAVEAT: the quota check would need some kind of unique lock to prevent multiple
    // concurrent checks to succeed where they would otherwise fail if ran sequentially.

    let states = update_summaries(tx, vec![hash], UpdateSummaryReason::Other).await?;

    let mut old_approved = false;
    let mut new_approved = Vec::new();

    for (hash, state) in states {
        match state {
            NodeState::Complete => (),
            NodeState::Approved => {
                old_approved = true;
                continue;
            }
            NodeState::Incomplete | NodeState::Rejected => continue,
        }

        let approve = if let Some(quota) = quota {
            match quota::check(tx, &hash, quota).await {
                Ok(()) => true,
                Err(QuotaError::Exceeded(size)) => {
                    tracing::warn!(?hash, quota = %quota, size = %size, "snapshot rejected - quota exceeded");
                    false
                }
                Err(QuotaError::Outdated) => {
                    tracing::debug!(?hash, "snapshot outdated");
                    false
                }
                Err(QuotaError::Store(error)) => return Err(error),
            }
        } else {
            true
        };

        if approve {
            root_node::approve(tx, &hash).await?;
            try_collect_into(root_node::load_writer_ids(tx, &hash), &mut new_approved).await?;
        } else {
            root_node::reject(tx, &hash).await?;
        }
    }

    Ok(ReceiveStatus {
        old_approved,
        new_approved,
    })
}
