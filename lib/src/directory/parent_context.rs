use super::{Error, MissingBlockStrategy};
use crate::{
    blob::{self, BlobPin},
    blob_id::BlobId,
    branch::Branch,
    db,
    directory::{content::EntryExists, Directory},
    error::Result,
    index::VersionVectorOp,
    locator::Locator,
    version_vector::VersionVector,
};
use tracing::instrument;

/// Info about an entry in the context of its parent directory.
#[derive(Clone)]
pub(crate) struct ParentContext {
    // BlobId of the parent directory of the entry.
    directory_id: BlobId,
    // Pin of the parent directory to protect it from being garbage collected while this
    // `ParentContext` exists.
    directory_pin: BlobPin,
    // The name of the entry in its parent directory.
    entry_name: String,
    // ParentContext of the parent directory ("grandparent context")
    parent: Option<Box<Self>>,
}

impl ParentContext {
    pub(super) fn new(
        directory_id: BlobId,
        directory_pin: BlobPin,
        entry_name: String,
        parent: Option<Self>,
    ) -> Self {
        Self {
            directory_id,
            directory_pin,
            entry_name,
            parent: parent.map(Box::new),
        }
    }

    /// This updates the version vector of this entry and all its ancestors.
    pub async fn bump(
        &self,
        tx: &mut db::WriteTransaction,
        branch: Branch,
        op: VersionVectorOp<'_>,
    ) -> Result<()> {
        let mut directory = self.open_in(tx, branch).await?;
        let mut content = directory.entries.clone();
        content.bump(directory.branch(), &self.entry_name, op)?;
        directory.save(tx, &content).await?;
        directory.bump(tx, op).await?;

        Ok(())
    }

    /// Atomically forks the blob of this entry into the local branch and returns the updated
    /// parent context.
    #[instrument(
        skip_all,
        fields(
            name = self.entry_name,
            parent_id = ?self.directory_id,
            src_branch.id = ?src_branch.id(),
            dst_branch.id = ?dst_branch.id()),
        err(Debug)
    )]
    pub async fn fork(&self, src_branch: &Branch, dst_branch: &Branch) -> Result<Self> {
        let directory = self.open(src_branch.clone()).await?;
        let src_entry_data = directory.lookup(&self.entry_name)?.clone_data();
        let blob_id = *src_entry_data.blob_id().ok_or(Error::EntryNotFound)?;

        // Fork the parent directory first.
        let mut directory = directory.fork(dst_branch).await?;

        let new_context = Self {
            directory_id: *directory.locator().blob_id(),
            directory_pin: directory.pin.clone(),
            entry_name: self.entry_name.clone(),
            parent: directory.parent.clone().map(Box::new),
        };

        // Check whether the fork is allowed, to avoid the hard work in case it isn't.
        match directory
            .entries
            .check_insert(directory.branch(), self.entry_name(), &src_entry_data)
        {
            Ok(()) => (),
            Err(EntryExists::Same) => {
                // The entry was already forked concurrently. This is treated as OK to maintain
                // idempotency.
                return Ok(new_context);
            }
            Err(EntryExists::Different | EntryExists::Concurrent | EntryExists::Open) => {
                return Err(Error::EntryExists);
            }
        }

        // Pin the blob so that it's not garbage collected prematurely.
        let _pin = dst_branch.pin_blob_for_collect(blob_id);

        // Fork the blob first without inserting it into the dst directory. This is because
        // `blob::fork` is not atomic and in case it's interrupted, we don't want overwrite the dst
        // entry yet. This makes the blob temporarily unreachable but it's OK because we pinned it
        // earlier.
        blob::fork(blob_id, src_branch, dst_branch).await?;

        // Now atomically insert the blob entry into the dst directory. If this fails, the
        // previously forked blob is unpinned and eventually garbage collected. This should however
        // be rare because we checked whether the insert can be done earlier, before forking the
        // blob. It can happen if the dst entry was modified concurrently while the blob was being
        // forked.
        let mut tx = directory.branch().db().begin_write().await?;
        let mut content = directory.load(&mut tx).await?;
        let src_vv = src_entry_data.version_vector().clone();

        match content.insert(directory.branch(), self.entry_name.clone(), src_entry_data) {
            Ok(()) => {
                directory.save(&mut tx, &content).await?;
                directory
                    .commit(tx, content, VersionVectorOp::Merge(&src_vv))
                    .await?;
            }
            Err(EntryExists::Same) => {
                // The entry was already forked concurrently. This is treated as OK to maintain
                // idempotency.
            }
            Err(EntryExists::Different | EntryExists::Concurrent | EntryExists::Open) => {
                // Dst entry was modified concurrently in an incompatible way and can't be
                // overwritten.
                return Err(Error::EntryExists);
            }
        };

        Ok(new_context)
    }

    pub(super) fn entry_name(&self) -> &str {
        &self.entry_name
    }

    /// Returns the version vector of this entry.
    pub async fn entry_version_vector(&self, branch: Branch) -> Result<VersionVector> {
        Ok(self
            .open(branch)
            .await?
            .lookup(&self.entry_name)?
            .version_vector()
            .clone())
    }

    /// Opens the parent directory of this entry.
    pub async fn open(&self, branch: Branch) -> Result<Directory> {
        Directory::open(
            branch,
            Locator::head(self.directory_id),
            self.parent.as_deref().cloned(),
            MissingBlockStrategy::Fail,
        )
        .await
    }

    /// Opens the parent directory of this entry.
    async fn open_in(&self, tx: &mut db::ReadTransaction, branch: Branch) -> Result<Directory> {
        Directory::open_in(
            self.directory_pin.clone(),
            tx,
            branch,
            Locator::head(self.directory_id),
            self.parent.as_deref().cloned(),
            MissingBlockStrategy::Fail,
        )
        .await
    }
}
