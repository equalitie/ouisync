use super::{content::InsertError, Error, MissingBlockStrategy};
use crate::{
    blob::{self, lock::ReadLock},
    blob_id::BlobId,
    branch::Branch,
    db,
    directory::{content::EntryExists, Directory},
    error::Result,
    index::VersionVectorOp,
    locator::Locator,
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
    directory_lock: ReadLock,
    // The name of the entry in its parent directory.
    entry_name: String,
    // ParentContext of the parent directory ("grandparent context")
    parent: Option<Box<Self>>,
}

impl ParentContext {
    pub(super) fn new(
        directory_id: BlobId,
        directory_lock: ReadLock,
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
    // TODO: move this function to the `file` mod.
    #[instrument(
        skip_all,
        fields(
            name = self.entry_name,
            parent_id = ?self.directory_id,
            src_branch.id = ?src_branch.id(),
            dst_branch.id = ?dst_branch.id(),
            blob_id,
        ),
        err(Debug)
    )]
    pub async fn fork(&self, src_branch: &Branch, dst_branch: &Branch) -> Result<Self> {
        let directory = self.open(src_branch.clone()).await?;
        let src_entry_data = directory.lookup(&self.entry_name)?.clone_data();

        let new_blob_id = *src_entry_data.blob_id().ok_or(Error::EntryNotFound)?;
        Span::current().record("blob_id", field::debug(&new_blob_id));

        // Fork the parent directory first.
        let mut directory = directory.fork(dst_branch).await?;

        let new_context = Self {
            directory_id: *directory.locator().blob_id(),
            directory_lock: directory.lock.clone(),
            entry_name: self.entry_name.clone(),
            parent: directory.parent.clone().map(Box::new),
        };

        // Check whether the fork is allowed, to avoid the hard work in case it isn't.
        let old_blob_id = match directory
            .entries
            .check_insert(self.entry_name(), &src_entry_data)
        {
            Ok(id) => id,
            Err(EntryExists::Same) => {
                // The entry was already forked concurrently. This is treated as OK to maintain
                // idempotency.
                return Ok(new_context);
            }
            Err(EntryExists::Different) => {
                return Err(Error::EntryExists);
            }
        };

        // Try to acquire a remove lock for the old entry (if any). If no one is currently accessing
        // the old entry, this will succeeds and no one will be able to access it more until this
        // function completes or is cancelled. If the old blob id is the same as the new one, this
        // also protects the new blob from being garbage collected until it's inserted into the
        // dst directory. If they are different, we'll create a separate lock for the new blob
        // later.
        let old_lock = if let Some(old_blob_id) = old_blob_id {
            Some(
                dst_branch
                    .locker()
                    .remove_wait(old_blob_id)
                    .await
                    .ok_or(Error::Locked)?,
            )
        } else {
            None
        };

        // If we acquired the lock we need to reload the directory and check again. This prevents
        // race condition where the old entry might have been modified after we forked the directory
        // but before we did the first check. This also avoids us trying to fork the blob again if
        // if was already forked by someone else in the meantime.
        if old_lock.is_some() {
            directory.refresh().await?;

            match directory
                .entries
                .check_insert(self.entry_name(), &src_entry_data)
            {
                Ok(_) => (),
                Err(EntryExists::Same) => return Ok(new_context),
                Err(EntryExists::Different) => return Err(Error::EntryExists),
            }
        }

        // If the new and old blob ids are different, create a separate (read) lock for the new one
        // as well so that it's not garbage collected prematurely.
        let _new_lock = if Some(new_blob_id) != old_blob_id {
            Some(dst_branch.locker().read(new_blob_id).ok_or(Error::Locked)?)
        } else {
            None
        };

        // Fork the blob first without inserting it into the dst directory. This is because
        // `blob::fork` is not atomic and in case it's interrupted, we don't want overwrite the dst
        // entry yet. This makes the blob temporarily unreachable but it's OK because we locked it.
        blob::fork(new_blob_id, src_branch, dst_branch).await?;

        // Now atomically insert the blob entry into the dst directory. This can still fail if
        // the dst entry didn't initially exist but was inserted by someone in the meantime. Also
        // this whole function can be cancelled before the insertion is completed. In both of these
        // cases the newly forked blob will be unlocked and eventually garbage-collected. This
        // wastes work but is otherwise harmless. The fork can be retried at any time.
        let mut tx = directory.branch().db().begin_write().await?;
        let mut content = directory.load(&mut tx).await?;
        let src_vv = src_entry_data.version_vector().clone();

        match content.insert(
            directory.branch(),
            self.entry_name.clone(),
            src_entry_data,
            old_lock,
        ) {
            Ok(_old_lock) => {
                directory.save(&mut tx, &content).await?;
                directory
                    .commit(tx, content, VersionVectorOp::Merge(&src_vv))
                    .await?;

                Ok(new_context)
            }
            Err(InsertError::Exists(EntryExists::Same)) => Ok(new_context),
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
            self.directory_lock.clone(),
            tx,
            branch,
            Locator::head(self.directory_id),
            self.parent.as_deref().cloned(),
            MissingBlockStrategy::Fail,
        )
        .await
    }
}
