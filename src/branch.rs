use crate::{
    crypto::Cryptor,
    db,
    directory::Directory,
    entry::{Entry, EntryType},
    error::{Error, Result},
    file::File,
    index::BranchData,
    locator::Locator,
    ReplicaId,
};
use camino::{Utf8Component, Utf8Path};

pub struct Branch {
    pool: db::Pool,
    branch_data: BranchData,
    cryptor: Cryptor,
}

impl Branch {
    pub fn new(pool: db::Pool, branch_data: BranchData, cryptor: Cryptor) -> Self {
        Self {
            pool,
            branch_data,
            cryptor,
        }
    }

    pub fn id(&self) -> &ReplicaId {
        self.branch_data.replica_id()
    }

    pub fn data(&self) -> &BranchData {
        &self.branch_data
    }

    /// Open an entry (file or directory) at the given locator.
    pub async fn open_entry_by_locator(
        &self,
        locator: Locator,
        entry_type: EntryType,
    ) -> Result<Entry> {
        match entry_type {
            EntryType::File => Ok(Entry::File(self.open_file_by_locator(locator).await?)),
            EntryType::Directory => Ok(Entry::Directory(
                self.open_directory_by_locator(locator).await?,
            )),
        }
    }

    pub async fn open_file_by_locator(&self, locator: Locator) -> Result<File> {
        File::open(
            self.pool.clone(),
            self.branch_data.clone(),
            self.cryptor.clone(),
            locator,
        )
        .await
    }

    pub async fn open_directory_by_locator(&self, locator: Locator) -> Result<Directory> {
        Directory::open(
            self.pool.clone(),
            self.branch_data.clone(),
            self.cryptor.clone(),
            locator,
        )
        .await
    }

    pub async fn ensure_root_exists(&self) -> Result<Directory> {
        match self.open_directory_by_locator(Locator::Root).await {
            Ok(dir) => Ok(dir),
            Err(Error::EntryNotFound) => Ok(Directory::create(
                self.pool.clone(),
                self.branch_data.clone(),
                self.cryptor.clone(),
                Locator::Root,
            )),
            Err(error) => Err(error),
        }
    }

    /// Ensures that the directory at the specified path exists including all its ancestors.
    /// Note: non-normalized paths (i.e. containing "..") or Windows-style drive prefixes
    /// (e.g. "C:") are not supported.
    pub async fn ensure_directory_exists(&self, path: &Utf8Path) -> Result<Vec<Directory>> {
        let mut dirs = vec![self.ensure_root_exists().await?];

        for component in path.components() {
            match component {
                Utf8Component::RootDir | Utf8Component::CurDir => (),
                Utf8Component::Normal(name) => {
                    let last = dirs.last_mut().unwrap();

                    let next = if let Ok(entry) = last.lookup_version(name, self.id()) {
                        entry.directory()?.open().await?
                    } else {
                        last.create_directory(name.to_string())?
                    };

                    dirs.push(next);
                }
                Utf8Component::Prefix(_) | Utf8Component::ParentDir => {
                    return Err(Error::OperationNotSupported)
                }
            }
        }

        Ok(dirs)
    }

    pub async fn ensure_file_exists(&self, path: &Utf8Path) -> Result<(File, Vec<Directory>)> {
        let (parent, name) = decompose_path(path).ok_or(Error::EntryIsDirectory)?;
        let mut dirs = self.ensure_directory_exists(parent).await?;
        let file = dirs.last_mut().unwrap().create_file(name.to_string())?;
        Ok((file, dirs))
    }
}

// Decomposes `Path` into parent and filename. Returns `None` if `path` doesn't have parent
// (it's the root).
fn decompose_path(path: &Utf8Path) -> Option<(&Utf8Path, &str)> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Some((parent, name)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, index::Index};

    #[tokio::test(flavor = "multi_thread")]
    async fn ensure_root_directory_exists() {
        let pool = db::init(db::Store::Memory).await.unwrap();
        let replica_id = rand::random();
        let index = Index::load(pool.clone(), replica_id).await.unwrap();
        let branch = Branch::new(pool, index.local_branch().await, Cryptor::Null);

        let dirs = branch.ensure_directory_exists("/".into()).await.unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].locator(), &Locator::Root);
    }
}
