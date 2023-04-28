use crate::{
    access_control::AccessKeys,
    blob::lock::{BranchLocker, Locker},
    block::BlockId,
    crypto::sign::PublicKey,
    db,
    debug::DebugPrinter,
    directory::{Directory, DirectoryFallback, DirectoryLocking, EntryRef},
    error::{Error, Result},
    event::{EventScope, EventSender, Payload},
    file::File,
    index::BranchData,
    locator::Locator,
    path,
    version_vector::VersionVector,
};
use camino::{Utf8Component, Utf8Path};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

#[derive(Clone)]
pub struct Branch {
    pool: db::Pool,
    branch_data: BranchData,
    keys: AccessKeys,
    shared: BranchShared,
    event_tx: EventSender,
}

impl Branch {
    pub(crate) fn new(
        pool: db::Pool,
        branch_data: BranchData,
        keys: AccessKeys,
        shared: BranchShared,
        event_tx: EventSender,
    ) -> Self {
        Self {
            pool,
            branch_data,
            keys,
            shared,
            event_tx,
        }
    }

    /// Binds the given event scope to this branch. Any event from this branch or any objects
    /// belonging to it (files, directories) will be sent with this scope.
    pub(crate) fn with_event_scope(self, event_scope: EventScope) -> Self {
        Self {
            event_tx: self.event_tx.with_scope(event_scope),
            ..self
        }
    }

    pub fn id(&self) -> &PublicKey {
        self.branch_data.id()
    }

    pub(crate) fn db(&self) -> &db::Pool {
        &self.pool
    }

    pub(crate) fn data(&self) -> &BranchData {
        &self.branch_data
    }

    pub async fn version_vector(&self) -> Result<VersionVector> {
        let mut conn = self.pool.acquire().await?;
        self.branch_data.load_version_vector(&mut conn).await
    }

    pub(crate) fn keys(&self) -> &AccessKeys {
        &self.keys
    }

    pub(crate) async fn open_root(
        &self,
        locking: DirectoryLocking,
        fallback: DirectoryFallback,
    ) -> Result<Directory> {
        Directory::open_root(self.clone(), locking, fallback).await
    }

    pub(crate) async fn open_or_create_root(&self) -> Result<Directory> {
        Directory::open_or_create_root(self.clone()).await
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
                    let next = match curr.lookup(name) {
                        Ok(EntryRef::Directory(entry)) => {
                            Some(entry.open(DirectoryFallback::Disabled).await?)
                        }
                        Ok(EntryRef::File(_)) => return Err(Error::EntryIsFile),
                        Ok(EntryRef::Tombstone(_)) | Err(Error::EntryNotFound) => None,
                        Err(error) => return Err(error),
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
        self.ensure_directory_exists(parent)
            .await?
            .create_file(name.to_string())
            .await
    }

    pub(crate) async fn root_block_id(&self) -> Result<BlockId> {
        let mut tx = self.pool.begin_read().await?;
        let (block_id, _) = self
            .data()
            .get(&mut tx, &Locator::ROOT.encode(self.keys().read()))
            .await?;
        Ok(block_id)
    }

    pub(crate) fn locker(&self) -> BranchLocker {
        self.shared.locker.branch(*self.id())
    }

    pub(crate) fn uncommitted_block_counter(&self) -> &Arc<AtomicCounter> {
        &self.shared.uncommitted_block_counter
    }

    pub(crate) fn notify(&self) -> BranchEventSender {
        BranchEventSender {
            event_tx: self.event_tx.clone(),
            branch_id: *self.id(),
        }
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        match self
            .open_root(DirectoryLocking::Disabled, DirectoryFallback::Disabled)
            .await
        {
            Ok(root) => root.debug_print(print).await,
            Err(error) => {
                print.display(&format_args!("failed to open root directory: {:?}", error))
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn reopen(self, keys: AccessKeys) -> Self {
        Self { keys, ..self }
    }
}

/// State shared among all branches.
#[derive(Clone)]
pub(crate) struct BranchShared {
    pub locker: Locker,
    // Number of blocks written without committing the shared transaction.
    pub uncommitted_block_counter: Arc<AtomicCounter>,
}

impl BranchShared {
    pub fn new() -> Self {
        Self {
            locker: Locker::new(),
            uncommitted_block_counter: Arc::new(AtomicCounter::new()),
        }
    }
}

pub(crate) struct AtomicCounter(AtomicU32);

impl AtomicCounter {
    pub fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    // Increments the counter by one and returns the new value.
    pub fn increment(&self) -> u32 {
        self.0
            .fetch_add(1, Ordering::Relaxed)
            .checked_add(1)
            .expect("counter out of range")
    }

    // Resets the counter back to zero.
    pub fn reset(&self) {
        self.0.store(0, Ordering::Relaxed)
    }
}

/// Sender to send event notification for the given branch.
#[derive(Clone)]
pub(crate) struct BranchEventSender {
    event_tx: EventSender,
    branch_id: PublicKey,
}

impl BranchEventSender {
    pub fn send(&self) {
        self.event_tx.send(Payload::BranchChanged(self.branch_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        access_control::WriteSecrets, db, event::EventSender, index::Index, locator::Locator,
        state_monitor::StateMonitor,
    };
    use assert_matches::assert_matches;
    use tempfile::TempDir;

    #[tokio::test(flavor = "multi_thread")]
    async fn ensure_root_directory_exists() {
        let (_base_dir, branch) = setup().await;
        let dir = branch.ensure_directory_exists("/".into()).await.unwrap();
        assert_eq!(dir.locator(), &Locator::ROOT);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ensure_subdirectory_exists() {
        let (_base_dir, branch) = setup().await;

        let mut root = branch.open_or_create_root().await.unwrap();

        branch
            .ensure_directory_exists(Utf8Path::new("/dir"))
            .await
            .unwrap();

        root.refresh().await.unwrap();
        let _ = root.lookup("dir").unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn attempt_to_modify_file_on_read_only_branch() {
        let (_base_dir, branch) = setup().await;

        let mut file = branch
            .open_or_create_root()
            .await
            .unwrap()
            .create_file("test.txt".into())
            .await
            .unwrap();
        file.write(b"foo").await.unwrap();
        file.flush().await.unwrap();
        drop(file);

        let keys = branch.keys().clone().read_only();
        let branch = branch.reopen(keys);

        let mut file = branch
            .open_root(DirectoryLocking::Enabled, DirectoryFallback::Disabled)
            .await
            .unwrap()
            .lookup("test.txt")
            .unwrap()
            .file()
            .unwrap()
            .open()
            .await
            .unwrap();

        file.truncate(0).await.unwrap();
        assert_matches!(file.flush().await, Err(Error::PermissionDenied));

        file.write(b"bar").await.unwrap();
        assert_matches!(file.flush().await, Err(Error::PermissionDenied));
    }

    async fn setup() -> (TempDir, Branch) {
        let monitor = StateMonitor::make_root();
        let (base_dir, pool) = db::create_temp(&monitor).await.unwrap();

        let writer_id = PublicKey::random();
        let secrets = WriteSecrets::random();
        let repository_id = secrets.id;
        let event_tx = EventSender::new(1);

        let index = Index::new(pool.clone(), repository_id, event_tx.clone());

        let shared = BranchShared::new();
        let data = index.get_branch(writer_id);
        let branch = Branch::new(pool, data, secrets.into(), shared, event_tx);

        (base_dir, branch)
    }
}
