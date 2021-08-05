use crate::{
    blob::Blob, branch::Branch, directory::Directory, entry_type::EntryType, error::Result,
    locator::Locator, path,
};
use camino::{Utf8Component, Utf8Path, Utf8PathBuf};

/// Context needed for updating all necessary info when writing to a file or directory.
pub struct WriteContext {
    path: Utf8PathBuf,
    local_branch: Branch,
    ancestors: Vec<Directory>,
}

impl WriteContext {
    pub fn new(path: Utf8PathBuf, local_branch: Branch) -> Self {
        Self {
            path,
            local_branch,
            ancestors: Vec::new(),
        }
    }

    pub fn child(&self, name: &str) -> Self {
        Self {
            path: self.path.join(name),
            local_branch: self.local_branch.clone(),
            ancestors: Vec::new(),
        }
    }

    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    pub fn local_branch(&self) -> &Branch {
        &self.local_branch
    }

    /// Begin writing to the given blob. This ensures the blob lives in the local branch and all
    /// its ancestor directories exist and live in the local branch as well.
    /// Call `commit` to finalize the write.
    pub async fn begin(&mut self, entry_type: EntryType, blob: &mut Blob) -> Result<()> {
        // TODO: load the directories always

        if blob.branch().id() == self.local_branch.id() {
            // Blob already lives in the local branch. We assume the ancestor directories have been
            // already created as well so there is nothing else to do.
            return Ok(());
        }

        let dst_locator = if let Some((parent, name)) = path::decompose(&self.path) {
            self.ancestors = self.local_branch.ensure_directory_exists(parent).await?;
            self.ancestors
                .last_mut()
                .unwrap()
                .insert_entry(name.to_owned(), entry_type)?
        } else {
            // `blob` is the root directory.
            Locator::Root
        };

        blob.fork(self.local_branch.data().clone(), dst_locator)
            .await
    }

    /// Commit writing to the blob started by a previous call to `begin`. Does nothing if `begin`
    /// was not called.
    pub async fn commit(&mut self) -> Result<()> {
        let mut dirs = self.ancestors.drain(..).rev();

        for component in self.path.components().rev() {
            match component {
                Utf8Component::Normal(name) => {
                    if let Some(mut dir) = dirs.next() {
                        dir.increment_entry_version(name)?;
                        dir.write().await?;
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
}

impl Clone for WriteContext {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            local_branch: self.local_branch.clone(),
            ancestors: Vec::new(), // The clone is produced in non-begun state.
        }
    }
}
