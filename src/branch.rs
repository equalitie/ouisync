use crate::{
    crypto::Cryptor,
    db,
    debug_printer::DebugPrinter,
    directory::{self, Directory, EntryRef, RootDirectoryCache},
    error::{Error, Result},
    file::File,
    index::BranchData,
    path, ReplicaId,
};
use camino::{Utf8Component, Utf8Path};
use futures_util::future;
use std::{cmp::Ordering, collections::VecDeque, iter, sync::Arc};

#[derive(Clone)]
pub struct Branch {
    pool: db::Pool,
    branch_data: Arc<BranchData>,
    cryptor: Cryptor,
    root_directory: Arc<RootDirectoryCache>,
}

impl Branch {
    pub(crate) fn new(pool: db::Pool, branch_data: Arc<BranchData>, cryptor: Cryptor) -> Self {
        Self {
            pool,
            branch_data,
            cryptor,
            root_directory: Arc::new(RootDirectoryCache::new()),
        }
    }

    pub fn id(&self) -> &ReplicaId {
        self.branch_data.id()
    }

    pub(crate) fn data(&self) -> &BranchData {
        &self.branch_data
    }

    pub(crate) fn db_pool(&self) -> &db::Pool {
        &self.pool
    }

    pub(crate) fn cryptor(&self) -> &Cryptor {
        &self.cryptor
    }

    pub(crate) async fn open_root(&self, local_branch: Branch) -> Result<Directory> {
        self.root_directory.open(self.clone(), local_branch).await
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
                    let next = if let Ok(entry) = curr.read().await.lookup_version(name, self.id())
                    {
                        Some(entry.directory()?.open().await?)
                    } else {
                        None
                    };

                    let next = if let Some(next) = next {
                        next
                    } else {
                        curr.create_directory(name.to_string()).await?
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
        dir.create_file(name.to_string()).await
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        // TODO: We're lying here about the local branch argument to open_root.
        if let Ok(root) = self.open_root(self.clone()).await {
            root.debug_print(print).await;
        }
    }
}

pub(crate) async fn merge(local: Branch, remote: Branch) -> Result<()> {
    assert_ne!(local.id(), remote.id());

    if *local.branch_data.versions().await > *remote.branch_data.versions().await {
        // Local newer than remote, nothing to merge
        return Ok(());
    }

    log::info!("merge {:?}", remote.id());

    let local_root = local.open_or_create_root().await?;
    let remote_root = remote.open_root(local.clone()).await?;
    let mut merger = DirectoryMerger::new(local_root, remote_root);

    while merger.step().await? {}

    Ok(())
}

struct DirectoryMerger {
    // queue of (loca, remote) directory pairs to merge.
    queue: VecDeque<(Directory, Directory)>,
    files_to_insert: Vec<(ReplicaId, File)>,
    directories_to_insert: Vec<String>,
}

impl DirectoryMerger {
    fn new(local_root: Directory, remote_root: Directory) -> Self {
        Self {
            queue: iter::once((local_root, remote_root)).collect(),
            files_to_insert: Vec::new(),
            directories_to_insert: Vec::new(),
        }
    }

    async fn step(&mut self) -> Result<bool> {
        let (local_dir, remote_dir) = if let Some(pair) = self.queue.pop_back() {
            pair
        } else {
            return Ok(false);
        };

        self.scan(&local_dir, &remote_dir).await?;
        self.apply(&local_dir).await?;

        Ok(true)
    }

    async fn scan(&mut self, local: &Directory, remote: &Directory) -> Result<()> {
        let (local, remote) = future::join(local.read(), remote.read()).await;

        for remote_entry in remote.entries() {
            self.scan_entry(&local, remote_entry).await?;
        }

        Ok(())
    }

    async fn scan_entry(
        &mut self,
        local: &directory::Reader<'_>,
        remote_entry: EntryRef<'_>,
    ) -> Result<()> {
        let local_entries = match local.lookup(remote_entry.name()) {
            Ok(local_entries) => local_entries,
            Err(Error::EntryNotFound) => {
                self.insert(remote_entry).await?;
                return Ok(());
            }
            Err(error) => return Err(error),
        };

        let mut insert = false;

        for local_entry in local_entries {
            match local_entry
                .version_vector()
                .partial_cmp(remote_entry.version_vector())
            {
                Some(Ordering::Less) => {
                    insert = true;
                }
                Some(Ordering::Equal) => unreachable!(),
                Some(Ordering::Greater) => return Ok(()),
                None => {
                    insert = true;
                }
            }
        }

        if insert {
            self.insert(remote_entry).await?;
        }

        Ok(())
    }

    async fn insert(&mut self, entry: EntryRef<'_>) -> Result<()> {
        match entry {
            EntryRef::File(entry) => self
                .files_to_insert
                .push((*entry.author(), entry.open().await?)),
            EntryRef::Directory(entry) => self.directories_to_insert.push(entry.name().to_owned()),
        }

        Ok(())
    }

    async fn apply(&mut self, _local: &Directory) -> Result<()> {
        // TODO
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, index::Index, locator::Locator};

    #[tokio::test(flavor = "multi_thread")]
    async fn ensure_root_directory_exists() {
        let branch = setup().await;
        let dir = branch.ensure_directory_exists("/".into()).await.unwrap();
        assert_eq!(dir.read().await.locator(), &Locator::Root);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ensure_subdirectory_exists() {
        let branch = setup().await;
        let root = branch.open_or_create_root().await.unwrap();
        root.flush().await.unwrap();

        let dir = branch
            .ensure_directory_exists(Utf8Path::new("/dir"))
            .await
            .unwrap();
        dir.flush().await.unwrap();

        let _ = root
            .read()
            .await
            .lookup_version("dir", branch.id())
            .unwrap();
    }

    async fn setup() -> Branch {
        let pool = db::init(db::Store::Memory).await.unwrap();
        let replica_id = rand::random();
        let index = Index::load(pool.clone(), replica_id).await.unwrap();
        let branch = Branch::new(pool, index.branches().await.local().clone(), Cryptor::Null);

        branch
    }
}
