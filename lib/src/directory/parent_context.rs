use super::{DirectoryFallback, Error};
use crate::{
    blob::BlobId,
    blob::{
        self,
        lock::{LockKind, ReadLock},
    },
    branch::Branch,
    directory::{content::EntryExists, Directory},
    error::Result,
    protocol::{Bump, Locator},
    store::{Changeset, ReadTransaction},
    version_vector::VersionVector,
};
use tracing::{field, instrument, Span};

/// Info about an entry in the context of its parent directory.
#[derive(Clone)]
pub(crate) struct ParentContext {
    // BlobId of the parent directory of the entry.
    directory_id: BlobId,
    // ReadLock of the parent directory to protect it from being garbage collected while this
    // `ParentContext` exists.
    directory_lock: Option<ReadLock>,
    // The name of the entry in its parent directory.
    entry_name: String,
    // ParentContext of the parent directory ("grandparent context")
    parent: Option<Box<Self>>,
}

impl ParentContext {
    pub(super) fn new(
        directory_id: BlobId,
        directory_lock: Option<ReadLock>,
        entry_name: String,
        parent: Option<Self>,
    ) -> Self {
        Self {
            directory_id,
            directory_lock,
            entry_name,
            parent: parent.map(Box::new),
        }
    }

    /// Updates the version vector of this entry and all its ancestors.
    ///
    /// Note: If `bump` is empty, it increments the version corresponding to `branch`.
    pub async fn bump(
        &self,
        tx: &mut ReadTransaction,
        changeset: &mut Changeset,
        branch: Branch,
        bump: Bump,
    ) -> Result<()> {
        let mut directory = self.open_in(tx, branch).await?;
        let mut content = directory.content.clone();
        let diff = content.bump(&self.entry_name, bump)?;
        directory.save(tx, changeset, &content).await?;
        directory.bump(tx, changeset, Bump::Add(diff)).await?;

        Ok(())
    }

    /// Atomically forks the blob of this entry into the local branch and returns the updated
    /// parent context.
    // TODO: move this function to the `file` mod.
    #[instrument(
        skip_all,
        fields(
            src_branch_id = ?src_branch.id(),
            blob_id,
        ),
        err(Debug)
    )]
    pub async fn fork(&self, src_branch: &Branch, dst_branch: &Branch) -> Result<Self> {
        let directory = self.open(src_branch.clone()).await?;
        let src_entry_data = directory.lookup(&self.entry_name)?.clone_data();
        let new_blob_id = *src_entry_data.blob_id().ok_or(Error::EntryNotFound)?;
        Span::current().record("blob_id", field::debug(&new_blob_id));

        tracing::trace!("fork started");

        // Fork the parent directory first.
        let mut directory = directory.fork(dst_branch).await?;

        let new_context = Self {
            directory_id: *directory.blob_id(),
            directory_lock: directory.lock.clone(),
            entry_name: self.entry_name.clone(),
            parent: directory.parent.clone().map(Box::new),
        };

        // Check whether the fork is allowed, to avoid the hard work in case it isn't.
        let old_blob_id = match directory
            .content
            .check_insert(self.entry_name(), &src_entry_data)
        {
            Ok(id) => id,
            Err(EntryExists::Same) => {
                // The entry was already forked concurrently. This is treated as OK to maintain
                // idempotency.
                tracing::trace!("already forked");
                return Ok(new_context);
            }
            Err(EntryExists::Different) => return Err(Error::EntryExists),
        };

        // Acquire unique lock for the destination blob. This prevents us from overwriting it if it
        // exists and is currently being accessed and it also coordinates multiple concurrent fork
        // of the same blob.
        let lock_blob_id = old_blob_id.unwrap_or(new_blob_id);
        let lock = loop {
            match dst_branch.locker().try_unique(lock_blob_id) {
                Ok(lock) => break Ok(lock),
                Err((notify, LockKind::Unique)) => notify.await,
                Err((_, LockKind::Read | LockKind::Write)) => break Err(Error::Locked),
            }
        };

        // We need to reload the directory and check again. This prevents race condition where the
        // old entry might have been modified after we forked the directory but before we did the
        // first check. This also avoids us trying to fork the blob again if if was already forked
        // by someone else in the meantime.
        directory.refresh().await?;

        match directory
            .content
            .check_insert(self.entry_name(), &src_entry_data)
        {
            Ok(_) => {
                // TODO: what if the old_blob_id changed since the first `check_insert`?
                // Can it happen? If so, it would currently cause panic in the `insert` below.
            }
            Err(EntryExists::Same) => {
                tracing::trace!("already forked");
                return Ok(new_context);
            }
            Err(EntryExists::Different) => return Err(Error::EntryExists),
        }

        let _lock = lock?;

        // Fork the blob first without inserting it into the dst directory. This is because
        // `blob::fork` is not atomic and in case it's interrupted, we don't want overwrite the dst
        // entry yet. This makes the blob temporarily unreachable but it's OK because we locked it.
        blob::fork(new_blob_id, src_branch, dst_branch).await?;

        // Now atomically insert the blob entry into the dst directory. This can still fail if
        // the dst entry didn't initially exist but was inserted by someone in the meantime. Also
        // this whole function can be cancelled before the insertion is completed. In both of these
        // cases the newly forked blob will be unlocked and eventually garbage-collected. This
        // wastes work but is otherwise harmless. The fork can be retried at any time.
        let mut tx = directory.branch().store().begin_write().await?;
        let mut changeset = Changeset::new();

        directory.refresh_in(&mut tx).await?;

        let mut content = directory.content.clone();

        match content.insert(self.entry_name.clone(), src_entry_data) {
            Ok(diff) => {
                directory.save(&mut tx, &mut changeset, &content).await?;
                directory
                    .bump(&mut tx, &mut changeset, Bump::Add(diff))
                    .await?;
                directory.commit(tx, changeset).await?;
                directory.finalize(content);
                tracing::trace!("fork complete");
                Ok(new_context)
            }
            Err(EntryExists::Same) => {
                tracing::trace!("already forked");
                Ok(new_context)
            }
            Err(error) => Err(error.into()),
        }
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
        let mut tx = branch.store().begin_read().await?;
        self.open_in(&mut tx, branch).await
    }

    /// Opens the parent directory of this entry.
    pub async fn open_in(&self, tx: &mut ReadTransaction, branch: Branch) -> Result<Directory> {
        Directory::open_in(
            self.directory_lock.as_ref().cloned(),
            tx,
            branch,
            Locator::head(self.directory_id),
            self.parent.as_deref().cloned(),
            DirectoryFallback::Disabled,
        )
        .await
    }
}
