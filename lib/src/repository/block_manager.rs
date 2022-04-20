//! Takes care of requesting missing blocks and removing unneeded blocks.

use super::Shared;
use crate::{
    blob::BlockIds,
    blob_id::BlobId,
    block,
    directory::EntryRef,
    error::{Error, Result},
    joint_directory::versioned,
    JointDirectory,
};
use async_recursion::async_recursion;
use futures_util::{stream, StreamExt};
use std::sync::Arc;

pub(super) async fn run(shared: Arc<Shared>) {
    let mut rx = shared.index.subscribe();

    while rx.recv().await.is_ok() {
        match handle(shared.as_ref()).await {
            Ok(()) => (),
            Err(error) => {
                log::error!("block manager failed: {:?}", error);
            }
        }
    }
}

async fn handle(shared: &Shared) -> Result<()> {
    let branches = shared.branches().await?;
    let mut roots = Vec::with_capacity(branches.len());

    for branch in branches {
        match branch.open_root().await {
            Ok(dir) => roots.push(dir),
            Err(Error::BlockNotFound(_)) => {
                // TODO: request the missing block
                continue;
            }
            Err(error) => return Err(error),
        }
    }

    traverse(shared, JointDirectory::new(None, roots)).await
}

#[async_recursion]
async fn traverse(shared: &Shared, dir: JointDirectory) -> Result<()> {
    for (_, entry_versions) in dir.read().await.entries_all_versions() {
        // Process children first.
        recurse(shared, &entry_versions).await?;

        let (current, outdated): (_, Vec<_>) = versioned::partition(entry_versions, None);

        for entry in current {
            request_missing_blocks(shared, entry).await?;
        }

        for entry in outdated {
            remove_unneeded_blocks(shared, entry).await?;
        }
    }

    Ok(())
}

async fn recurse(shared: &Shared, versions: &[EntryRef<'_>]) -> Result<()> {
    let dir_entries = versions.iter().filter_map(|entry| entry.directory().ok());
    let dirs: Vec<_> = stream::iter(dir_entries)
        .filter_map(|entry| async move { entry.open().await.ok() })
        .collect()
        .await;

    traverse(shared, JointDirectory::new(None, dirs)).await
}

async fn request_missing_blocks(_shared: &Shared, _entry: EntryRef<'_>) -> Result<()> {
    // TODO
    Ok(())
}

async fn remove_unneeded_blocks(shared: &Shared, entry: EntryRef<'_>) -> Result<()> {
    let blob_id = if let Some(blob_id) = blob_id(&entry) {
        blob_id
    } else {
        return Ok(());
    };

    let mut conn = shared.index.pool.acquire().await?;
    let mut block_ids = BlockIds::new(entry.branch(), *blob_id);

    while let Some(block_id) = block_ids.next(&mut conn).await? {
        block::remove(&mut conn, &block_id).await?;

        // TODO: consider releasing and re-acquiring the connection after some number of iterations,
        // to not hold it for too long.
    }

    Ok(())
}

fn blob_id<'a>(entry: &EntryRef<'a>) -> Option<&'a BlobId> {
    match entry {
        EntryRef::File(entry) => Some(entry.blob_id()),
        EntryRef::Directory(entry) => Some(entry.blob_id()),
        EntryRef::Tombstone(_) => None,
    }
}
