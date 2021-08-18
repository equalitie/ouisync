use crate::{
    blob::Blob,
    branch::Branch,
    directory::{Directory, EntryData},
    entry_type::EntryType,
    error::Result,
    locator::Locator,
    path,
    replica_id::ReplicaId,
    version_vector::VersionVector,
};
use camino::{Utf8Component, Utf8PathBuf};
use std::ops::DerefMut;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Context needed for updating all necessary info when writing to a file or directory.
pub struct WriteContext {
    local_branch_id: ReplicaId,
    // None iff this WriteContext corresponds to the root directory.
    parent: Option<Parent>,
    inner: Mutex<Inner>,
}

struct Parent {
    name: String,
    write_context: Arc<WriteContext>,
    // TODO: Should this be std::sync::Weak?
    entry: Arc<EntryData>,
}

struct Inner {
    local_branch: Branch,
    ancestors: Vec<Directory>,
}

impl WriteContext {
    pub fn new_for_root(local_branch: Branch) -> Arc<Self> {
        Arc::new(Self {
            local_branch_id: *local_branch.id(),
            parent: None,
            inner: Mutex::new(Inner {
                local_branch,
                ancestors: Vec::new(),
            }),
        })
    }

    pub async fn child(self: &Arc<Self>, name: String, parent_entry: Arc<EntryData>) -> Arc<Self> {
        let inner = self.inner.lock().await;

        Arc::new(Self {
            local_branch_id: self.local_branch_id,
            parent: Some(Parent {
                name,
                write_context: self.clone(),
                entry: parent_entry,
            }),
            inner: Mutex::new(Inner {
                local_branch: inner.local_branch.clone(),
                ancestors: Vec::new(),
            }),
        })
    }

    pub async fn local_branch(&self) -> Branch {
        self.inner.lock().await.local_branch.clone()
    }

    pub fn local_branch_id(&self) -> &ReplicaId {
        &self.local_branch_id
    }

    pub fn parent_entry(&self) -> Option<&Arc<EntryData>> {
        self.parent.as_ref().map(|parent| &parent.entry)
    }

    /// Begin writing to the given blob. This ensures the blob lives in the local branch and all
    /// its ancestor directories exist and live in the local branch as well.
    /// Call `commit` to finalize the write.
    pub async fn begin(&self, entry_type: EntryType, blob: &mut Blob) -> Result<()> {
        // TODO: load the directories always

        let mut guard = self.inner.lock().await;
        let inner = guard.deref_mut();

        if blob.branch().id() == inner.local_branch.id() {
            // Blob already lives in the local branch. We assume the ancestor directories have been
            // already created as well so there is nothing else to do.
            return Ok(());
        }

        let dst_locator = if let Some((parent, name)) = path::decompose(&self.calculate_path()) {
            inner.ancestors = inner.local_branch.ensure_directory_exists(parent).await?;
            let vv = self.version_vector().clone();
            inner
                .ancestors
                .last_mut()
                .unwrap()
                .insert_entry(name.to_owned(), entry_type, vv)
                .await?
                .locator()
        } else {
            // `blob` is the root directory.
            Locator::Root
        };

        blob.fork(inner.local_branch.clone(), dst_locator).await
    }

    /// Commit writing to the blob started by a previous call to `begin`. Does nothing if `begin`
    /// was not called.
    pub async fn commit(&self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let inner = guard.deref_mut();

        let mut dirs = inner.ancestors.drain(..).rev();

        for component in self.calculate_path().components().rev() {
            match component {
                Utf8Component::Normal(name) => {
                    if let Some(dir) = dirs.next() {
                        dir.increment_entry_version(name).await?;
                        dir.apply().await?;
                    } else {
                        break;
                    }
                }
                Utf8Component::Prefix(_) | Utf8Component::RootDir | Utf8Component::CurDir => (),
                Utf8Component::ParentDir => panic!("non-normalized paths not supported"),
            }
        }

        Ok(())
    }

    fn calculate_path(&self) -> Utf8PathBuf {
        match &self.parent {
            None => "/".into(),
            Some(parent) => parent.write_context.calculate_path().join(&parent.name),
        }
    }

    fn version_vector(&self) -> &VersionVector {
        // TODO: How do we get the VV when this WriteContext corresponds to the root directory?
        self.parent.as_ref().unwrap().entry.version_vector()
    }

    // For debugging
    pub async fn set_local_branch(&self, local_branch: Branch) {
        self.inner.lock().await.local_branch = local_branch
    }
}
