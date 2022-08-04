use crate::{
    access_control::AccessKeys,
    blob::{BlobCache, MaybeInitShared},
    blob_id::BlobId,
    block::BlockId,
    crypto::sign::PublicKey,
    db,
    debug::DebugPrinter,
    directory::{Directory, EntryRef},
    error::{Error, Result},
    file::File,
    index::BranchData,
    locator::Locator,
    path,
    version_vector::VersionVector,
};
use camino::{Utf8Component, Utf8Path};
use std::sync::Arc;

#[derive(Clone)]
pub struct Branch {
    branch_data: Arc<BranchData>,
    keys: AccessKeys,
    blob_cache: Arc<BlobCache>,
}

impl Branch {
    pub(crate) fn new(
        branch_data: Arc<BranchData>,
        keys: AccessKeys,
        blob_cache: Arc<BlobCache>,
    ) -> Self {
        Self {
            branch_data,
            keys,
            blob_cache,
        }
    }

    pub fn id(&self) -> &PublicKey {
        self.branch_data.id()
    }

    pub(crate) fn data(&self) -> &BranchData {
        &self.branch_data
    }

    pub async fn version_vector(&self, conn: &mut db::Connection) -> Result<VersionVector> {
        self.branch_data.load_version_vector(conn).await
    }

    pub(crate) fn keys(&self) -> &AccessKeys {
        &self.keys
    }

    pub(crate) async fn open_root(&self, conn: &mut db::Connection) -> Result<Directory> {
        Directory::open_root(conn, self.clone()).await
    }

    pub(crate) async fn open_or_create_root(&self, conn: &mut db::Connection) -> Result<Directory> {
        Directory::open_or_create_root(conn, self.clone()).await
    }

    /// Ensures that the directory at the specified path exists including all its ancestors.
    /// Note: non-normalized paths (i.e. containing "..") or Windows-style drive prefixes
    /// (e.g. "C:") are not supported.
    pub(crate) async fn ensure_directory_exists(
        &self,
        tx: &mut db::Transaction<'_>,
        path: &Utf8Path,
    ) -> Result<Directory> {
        let mut curr = self.open_or_create_root(tx).await?;

        for component in path.components() {
            match component {
                Utf8Component::RootDir | Utf8Component::CurDir => (),
                Utf8Component::Normal(name) => {
                    let next = match curr.lookup(name) {
                        Ok(EntryRef::Directory(entry)) => Some(entry.open(tx).await?),
                        Ok(EntryRef::File(_)) => return Err(Error::EntryIsFile),
                        Ok(EntryRef::Tombstone(_)) | Err(Error::EntryNotFound) => None,
                        Err(error) => return Err(error),
                    };

                    let next = if let Some(next) = next {
                        next
                    } else {
                        curr.create_directory(tx, name.to_string()).await?
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

    pub(crate) async fn ensure_file_exists(
        &self,
        tx: &mut db::Transaction<'_>,
        path: &Utf8Path,
    ) -> Result<File> {
        let (parent, name) = path::decompose(path).ok_or(Error::EntryIsDirectory)?;
        let mut dir = self.ensure_directory_exists(tx, parent).await?;
        dir.create_file(tx, name.to_string()).await
    }

    pub(crate) async fn root_block_id(&self, conn: &mut db::Connection) -> Result<BlockId> {
        self.data()
            .get(conn, &Locator::ROOT.encode(self.keys().read()))
            .await
    }

    pub(crate) fn fetch_blob_shared(&self, blob_id: BlobId) -> MaybeInitShared {
        self.blob_cache.fetch(*self.id(), blob_id)
    }

    pub(crate) fn is_blob_open(&self, blob_id: &BlobId) -> bool {
        self.blob_cache.contains(self.id(), blob_id)
    }

    pub(crate) fn is_any_blob_open(&self) -> bool {
        self.blob_cache.contains_any(self.id())
    }

    pub async fn debug_print(&self, conn: &mut db::Connection, print: DebugPrinter) {
        match self.open_root(conn).await {
            Ok(root) => root.debug_print(conn, print).await,
            Err(error) => {
                print.display(&format_args!("failed to open root directory: {:?}", error))
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn reopen(self, keys: AccessKeys) -> Self {
        Self {
            branch_data: self.branch_data,
            keys,
            blob_cache: self.blob_cache,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        access_control::WriteSecrets,
        db,
        index::{Index, Proof, EMPTY_INNER_HASH},
        locator::Locator,
    };
    use tempfile::TempDir;
    use tokio::sync::broadcast;

    #[tokio::test(flavor = "multi_thread")]
    async fn ensure_root_directory_exists() {
        let (_base_dir, pool, branch) = setup().await;
        let mut tx = pool.begin_immediate().await.unwrap();
        let dir = branch
            .ensure_directory_exists(&mut tx, "/".into())
            .await
            .unwrap();
        assert_eq!(dir.locator(), &Locator::ROOT);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ensure_subdirectory_exists() {
        let (_base_dir, pool, branch) = setup().await;
        let mut tx = pool.begin_immediate().await.unwrap();

        let mut root = branch.open_or_create_root(&mut tx).await.unwrap();

        branch
            .ensure_directory_exists(&mut tx, Utf8Path::new("/dir"))
            .await
            .unwrap();

        root.refresh(&mut tx).await.unwrap();
        let _ = root.lookup("dir").unwrap();
    }

    async fn setup() -> (TempDir, db::Pool, Branch) {
        let (base_dir, pool) = db::create_temp().await.unwrap();

        let writer_id = PublicKey::random();
        let secrets = WriteSecrets::random();
        let repository_id = secrets.id;
        let (event_tx, _) = broadcast::channel(1);

        let index = Index::load(pool.clone(), repository_id, event_tx.clone())
            .await
            .unwrap();

        let proof = Proof::new(
            writer_id,
            VersionVector::new(),
            *EMPTY_INNER_HASH,
            &secrets.write_keys,
        );
        let branch = index.create_branch(proof).await.unwrap();
        let branch = Branch::new(branch, secrets.into(), Arc::new(BlobCache::new(event_tx)));

        (base_dir, pool, branch)
    }
}
