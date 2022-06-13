use crate::{
    access_control::AccessKeys,
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
    version_vector::VersionVector,
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

    pub async fn version_vector(&self) -> Result<VersionVector> {
        self.branch_data
            .load_version_vector(&mut *self.pool.acquire().await?)
            .await
    }

    pub(crate) fn keys(&self) -> &AccessKeys {
        &self.keys
    }

    pub(crate) async fn open_root(&self, conn: &mut db::Connection) -> Result<Directory> {
        self.root_directory.open(conn, self.clone()).await
    }

    #[deprecated = "use `open_or_create_root_in_connection` instead"]
    pub(crate) async fn open_or_create_root(&self) -> Result<Directory> {
        let mut conn = self.pool.acquire().await?;
        self.open_or_create_root_in_connection(&mut conn).await
    }

    pub(crate) async fn open_or_create_root_in_connection(
        &self,
        conn: &mut db::Connection,
    ) -> Result<Directory> {
        self.root_directory.open_or_create(conn, self.clone()).await
    }

    /// Ensures that the directory at the specified path exists including all its ancestors.
    /// Note: non-normalized paths (i.e. containing "..") or Windows-style drive prefixes
    /// (e.g. "C:") are not supported.
    pub(crate) async fn ensure_directory_exists(
        &self,
        conn: &mut db::Connection,
        path: &Utf8Path,
    ) -> Result<Directory> {
        let mut curr = self.open_or_create_root_in_connection(conn).await?;

        for component in path.components() {
            match component {
                Utf8Component::RootDir | Utf8Component::CurDir => (),
                Utf8Component::Normal(name) => {
                    let next = match curr.read().await.lookup(name) {
                        Ok(EntryRef::Directory(entry)) => Some(entry.open(conn).await?),
                        Ok(EntryRef::File(_)) => return Err(Error::EntryIsFile),
                        Ok(EntryRef::Tombstone(_)) | Err(Error::EntryNotFound) => None,
                        Err(error) => return Err(error),
                    };

                    let next = if let Some(next) = next {
                        next
                    } else {
                        curr.create_directory(conn, name.to_string()).await?
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
        conn: &mut db::Connection,
        path: &Utf8Path,
    ) -> Result<File> {
        let (parent, name) = path::decompose(path).ok_or(Error::EntryIsDirectory)?;
        let dir = self.ensure_directory_exists(conn, parent).await?;
        dir.create_file(conn, name.to_string()).await
    }

    pub(crate) async fn root_block_id(&self) -> Result<BlockId> {
        let mut conn = self.db_pool().acquire().await?;
        self.data()
            .get(&mut conn, &Locator::ROOT.encode(self.keys().read()))
            .await
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        match self.pool.acquire().await {
            Ok(mut conn) => match self.open_root(&mut conn).await {
                Ok(root) => root.debug_print(print).await,
                Err(error) => {
                    print.display(&format_args!("failed to open root directory: {:?}", error))
                }
            },
            Err(error) => print.display(&format_args!(
                "failed to open root directory - failed to acquire db connection: {:?}",
                error
            )),
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
    use crate::{
        access_control::WriteSecrets,
        db,
        index::{Index, Proof},
        locator::Locator,
    };

    #[tokio::test(flavor = "multi_thread")]
    async fn ensure_root_directory_exists() {
        let branch = setup().await;
        let mut conn = branch.db_pool().acquire().await.unwrap();
        let dir = branch
            .ensure_directory_exists(&mut conn, "/".into())
            .await
            .unwrap();
        assert_eq!(dir.read().await.locator(), &Locator::ROOT);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ensure_subdirectory_exists() {
        let branch = setup().await;
        let mut conn = branch.db_pool().acquire().await.unwrap();

        let root = branch
            .open_or_create_root_in_connection(&mut conn)
            .await
            .unwrap();

        branch
            .ensure_directory_exists(&mut conn, Utf8Path::new("/dir"))
            .await
            .unwrap();

        let _ = root.read().await.lookup("dir").unwrap();
    }

    async fn setup() -> Branch {
        let pool = db::create(&db::Store::Temporary).await.unwrap();

        let writer_id = PublicKey::random();
        let secrets = WriteSecrets::random();
        let repository_id = secrets.id;

        let index = Index::load(pool.clone(), repository_id).await.unwrap();

        let proof = Proof::first(writer_id, &secrets.write_keys);
        let branch = index.create_branch(proof).await.unwrap();

        Branch::new(pool, branch, secrets.into())
    }
}
