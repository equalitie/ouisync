use crate::{
    access_control::AccessKeys,
    blob::Blob,
    block::BlockId,
    db,
    debug_printer::DebugPrinter,
    directory::{Directory, EntryRef, RootDirectoryCache},
    error::{Error, Result},
    file::File,
    index::BranchData,
    locator::Locator,
    path,
    sign::PublicKey,
};
use camino::{Utf8Component, Utf8Path};
use std::sync::Arc;

#[derive(Clone)]
pub struct Branch {
    pool: db::Pool,
    branch_data: Arc<BranchData>,
    keys: AccessKeys,
    root_directory: Arc<RootDirectoryCache>,
}

impl Branch {
    pub(crate) fn new(pool: db::Pool, branch_data: Arc<BranchData>, keys: AccessKeys) -> Self {
        Self {
            pool,
            branch_data,
            keys,
            root_directory: Arc::new(RootDirectoryCache::new()),
        }
    }

    pub fn id(&self) -> &PublicKey {
        self.branch_data.id()
    }

    pub(crate) fn data(&self) -> &BranchData {
        &self.branch_data
    }

    pub(crate) fn db_pool(&self) -> &db::Pool {
        &self.pool
    }

    pub(crate) fn keys(&self) -> &AccessKeys {
        &self.keys
    }

    pub(crate) async fn open_root(&self) -> Result<Directory> {
        self.root_directory.open(self.clone()).await
    }

    pub(crate) async fn open_or_create_root(&self) -> Result<Directory> {
        self.root_directory.open_or_create(self.clone()).await
    }

    /// Ensures that the directory at the specified path exists including all its ancestors.
    /// Note: non-normalized paths (i.e. containing "..") or Windows-style drive prefixes
    /// (e.g. "C:") are not supported.
    pub(crate) async fn ensure_directory_exists(&self, path: &Utf8Path) -> Result<Directory> {
        let mut curr = self.open_or_create_root().await?;

        for component in path.components() {
            match component {
                Utf8Component::RootDir | Utf8Component::CurDir => (),
                Utf8Component::Normal(name) => {
                    let next = match curr.read().await.lookup_version(name, self.id()) {
                        Ok(EntryRef::Directory(entry)) => Some(entry.open().await?),
                        Ok(EntryRef::File(_)) => return Err(Error::EntryIsFile),
                        Ok(EntryRef::Tombstone(_)) | Err(Error::EntryNotFound) => None,
                        Err(error) => return Err(error),
                    };

                    let next = if let Some(next) = next {
                        next
                    } else {
                        curr.create_directory(name.to_string(), self).await?
                    };

                    curr = next;
                }
                Utf8Component::Prefix(_) | Utf8Component::ParentDir => {
                    return Err(Error::OperationNotSupported)
                }
            }
        }

        Ok(curr)
    }

    pub(crate) async fn ensure_file_exists(&self, path: &Utf8Path) -> Result<File> {
        let (parent, name) = path::decompose(path).ok_or(Error::EntryIsDirectory)?;
        let dir = self.ensure_directory_exists(parent).await?;
        dir.create_file(name.to_string(), self).await
    }

    pub async fn root_block_id(&self) -> Result<BlockId> {
        let blob = Blob::open(self.clone(), Locator::ROOT).await?;
        blob.first_block_id().await
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        if let Ok(root) = self.open_root().await {
            root.debug_print(print).await;
        }
    }

    #[cfg(test)]
    pub(crate) fn reopen(self, keys: AccessKeys) -> Self {
        Self {
            pool: self.pool,
            branch_data: self.branch_data,
            keys,
            root_directory: self.root_directory,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{access_control::WriteSecrets, db, index::Index, locator::Locator, repository};

    #[tokio::test(flavor = "multi_thread")]
    async fn ensure_root_directory_exists() {
        let branch = setup().await;
        let dir = branch.ensure_directory_exists("/".into()).await.unwrap();
        assert_eq!(dir.read().await.locator(), &Locator::ROOT);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ensure_subdirectory_exists() {
        let branch = setup().await;
        let root = branch.open_or_create_root().await.unwrap();
        root.flush(None).await.unwrap();

        let dir = branch
            .ensure_directory_exists(Utf8Path::new("/dir"))
            .await
            .unwrap();
        dir.flush(None).await.unwrap();

        let _ = root
            .read()
            .await
            .lookup_version("dir", branch.id())
            .unwrap();
    }

    async fn setup() -> Branch {
        let pool = repository::create_db(&db::Store::Memory).await.unwrap();

        let writer_id = PublicKey::random();
        let secrets = WriteSecrets::random();
        let repository_id = secrets.id;
        let keys = secrets.into();

        let index = Index::load(pool.clone(), repository_id).await.unwrap();
        let branch = index.create_branch(writer_id).await.unwrap();
        Branch::new(pool, branch, keys)
    }
}
