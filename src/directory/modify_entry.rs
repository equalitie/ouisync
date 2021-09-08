use super::inner::{EntryData, Inner};
use crate::{db, error::Result, replica_id::ReplicaId, version_vector::VersionVector};
use std::collections::BTreeMap;
use tokio::sync::RwLockWriteGuard;

pub(super) struct ModifyEntry<'a, 'b> {
    steps: Vec<ModifyEntryStep<'a>>,
    local_id: &'a ReplicaId,
    tx: Option<db::Transaction<'b>>,
}

impl<'a, 'b> ModifyEntry<'a, 'b> {
    pub fn new(tx: db::Transaction<'b>, local_id: &'a ReplicaId) -> Self {
        Self {
            steps: Vec::new(),
            local_id,
            tx: Some(tx),
        }
    }

    pub fn add(&mut self, step: ModifyEntryStep<'a>) {
        self.steps.push(step)
    }

    pub async fn flush(&mut self) -> Result<()> {
        // `unwrap` is OK because `self.tx` is `Some` initially and we only set it to `None` in
        // `commit` which consumes `self` so this method cannot be called again.
        let tx = &mut self.tx.as_mut().unwrap();

        for step in &mut self.steps {
            step.inner.modify_entry(
                step.name,
                step.author_id,
                *self.local_id,
                step.version_vector_override,
            )?;
            step.inner.flush(tx).await?;
        }

        Ok(())
    }

    pub async fn commit(mut self) -> Result<()> {
        if let Some(tx) = self.tx.take() {
            tx.commit().await?;
        }

        Ok(())
    }
}

impl Drop for ModifyEntry<'_, '_> {
    fn drop(&mut self) {
        if self.tx.is_none() {
            return;
        }

        for step in self.steps.drain(..) {
            step.rollback()
        }
    }
}

pub(super) struct ModifyEntryStep<'a> {
    inner: RwLockWriteGuard<'a, Inner>,
    name: &'a str,
    author_id: &'a mut ReplicaId,
    version_vector_override: Option<&'a VersionVector>,
    author_id_backup: ReplicaId,
    versions_backup: Option<BTreeMap<ReplicaId, EntryData>>,
    dirty_backup: bool,
}

impl<'a> ModifyEntryStep<'a> {
    pub fn new(
        inner: RwLockWriteGuard<'a, Inner>,
        name: &'a str,
        author_id: &'a mut ReplicaId,
        version_vector_override: Option<&'a VersionVector>,
    ) -> Self {
        let author_id_backup = *author_id;
        let versions_backup = inner.content.entries.get(name).cloned();
        let dirty_backup = inner.content.dirty;

        Self {
            inner,
            name,
            author_id,
            version_vector_override,
            author_id_backup,
            versions_backup,
            dirty_backup,
        }
    }

    fn rollback(mut self) {
        *self.author_id = self.author_id_backup;
        self.inner.content.dirty = self.dirty_backup;

        if let Some(versions_backup) = self.versions_backup {
            // `unwrap` is OK here because the existence of `versions_backup` implies that the
            // entry for `name` exists because we never remove it during the `modify_entry` call.
            // Also as the `ModifyEntry` struct is holding an exclusive lock (write lock) to the
            // directory internals, it's impossible for someone to remove the entry in the meantime.
            *self.inner.content.entries.get_mut(self.name).unwrap() = versions_backup;
        } else {
            self.inner.content.entries.remove(self.name);
        }
    }
}
