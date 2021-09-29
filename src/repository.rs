use crate::{
    block,
    branch::Branch,
    crypto::Cryptor,
    db,
    debug_printer::DebugPrinter,
    directory::Directory,
    error::{Error, Result},
    file::File,
    index::{self, BranchData, Index, Subscription},
    joint_directory::{JointDirectory, JointEntryRef},
    joint_entry::JointEntryType,
    path,
    scoped_task::ScopedJoinHandle,
    ReplicaId,
};
use camino::Utf8Path;
use futures_util::{future, stream::FuturesUnordered, StreamExt};
use log::Level;
use std::{collections::HashMap, iter, sync::Arc};
use tokio::{select, sync::Mutex, task};

pub struct Repository {
    shared: Arc<Shared>,
    _merge_handle: Option<ScopedJoinHandle<()>>,
}

impl Repository {
    /// Opens an existing repository or creates a new one if it doesn't exists yet.
    pub async fn open(
        store: db::Store,
        this_replica_id: ReplicaId,
        cryptor: Cryptor,
        enable_merger: bool,
    ) -> Result<Self> {
        let pool = open_db(store).await?;
        let index = Index::load(pool, this_replica_id).await?;

        let shared = Arc::new(Shared {
            index,
            cryptor,
            branches: Mutex::new(HashMap::new()),
        });

        let merge_handle = if enable_merger {
            Some(ScopedJoinHandle(task::spawn(
                Merger::new(shared.clone()).run(),
            )))
        } else {
            None
        };

        Ok(Self {
            shared,
            _merge_handle: merge_handle,
        })
    }

    pub fn this_replica_id(&self) -> &ReplicaId {
        self.shared.index.this_replica_id()
    }

    /// Looks up an entry by its path. The path must be relative to the repository root.
    /// If the entry exists, returns its `JointEntryType`, otherwise returns `EntryNotFound`.
    pub async fn lookup_type<P: AsRef<Utf8Path>>(&self, path: P) -> Result<JointEntryType> {
        match path::utf8::decompose(path.as_ref()) {
            Some((parent, name)) => {
                let parent = self.open_directory(parent).await?;
                let parent = parent.read().await;
                Ok(parent.lookup_unique(name)?.entry_type())
            }
            None => Ok(JointEntryType::Directory),
        }
    }

    /// Opens a file at the given path (relative to the repository root)
    pub async fn open_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<File> {
        let (parent, name) = path::utf8::decompose(path.as_ref()).ok_or(Error::EntryIsDirectory)?;

        self.open_directory(parent)
            .await?
            .read()
            .await
            .lookup_unique(name)?
            .file()?
            .open()
            .await
    }

    /// Open a specific version of the file at the given path.
    pub async fn open_file_version<P: AsRef<Utf8Path>>(
        &self,
        path: P,
        branch_id: &ReplicaId,
    ) -> Result<File> {
        let (parent, name) = path::utf8::decompose(path.as_ref()).ok_or(Error::EntryIsDirectory)?;
        self.open_directory(parent)
            .await?
            .read()
            .await
            .lookup_version(name, branch_id)?
            .open()
            .await
    }

    /// Opens a directory at the given path (relative to the repository root)
    pub async fn open_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<JointDirectory> {
        self.joint_root().await?.cd(path).await
    }

    /// Creates a new file at the given path.
    pub async fn create_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<File> {
        self.local_branch()
            .await
            .ensure_file_exists(path.as_ref())
            .await
    }

    /// Creates a new directory at the given path.
    pub async fn create_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<Directory> {
        self.local_branch()
            .await
            .ensure_directory_exists(path.as_ref())
            .await
    }

    /// Removes (delete) the file at the given path. Returns the parent directory.
    pub async fn remove_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<JointDirectory> {
        let (parent, name) = path::utf8::decompose(path.as_ref()).ok_or(Error::EntryIsDirectory)?;
        let mut dir = self.open_directory(parent).await?;
        dir.remove_file(name).await?;
        Ok(dir)
    }

    /// Removes the directory at the given path. The directory must be empty. Returns the parent
    /// directory.
    pub async fn remove_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<JointDirectory> {
        let (parent, name) =
            path::utf8::decompose(path.as_ref()).ok_or(Error::OperationNotSupported)?;
        let parent = self.open_directory(parent).await?;
        // TODO: Currently only removing directories from the local branch is supported. To
        // implement removing a directory from another branches we need to introduce tombstones.
        parent
            .remove_directory(self.this_replica_id(), name)
            .await?;
        Ok(parent)
    }

    /// Moves (renames) an entry from the source path to the destination path.
    /// If both source and destination refer to the same entry, this is a no-op.
    /// Returns the parent directories of both `src` and `dst`.
    pub async fn move_entry<S: AsRef<Utf8Path>, D: AsRef<Utf8Path>>(
        &self,
        src_dir_path: S,
        src_name: &str,
        dst_dir_path: D,
        dst_name: &str,
    ) -> Result<()> {
        let src_joint_dir = self.open_directory(src_dir_path).await?;
        let src_joint_reader = src_joint_dir.read().await;

        let (src_dir, src_name, src_author) = match src_joint_reader.lookup_unique(src_name)? {
            JointEntryRef::File(entry) => {
                let src_name = entry.name().to_string();
                let src_author = *entry.author();

                let mut file = entry.open().await?;

                drop(src_joint_reader);
                drop(src_joint_dir);

                file.fork().await?;

                (file.parent(), src_name, src_author)
            }
            JointEntryRef::Directory(_) => todo!(),
        };

        let dst_joint_dir = self.open_directory(&dst_dir_path).await?;
        let dst_joint_reader = dst_joint_dir.read().await;

        let dst_vv = match dst_joint_reader.lookup_unique(dst_name) {
            Err(Error::EntryNotFound) => {
                // Even if there is no regular entry, there still may be tombstones and so the
                // destination version vector must be "happened after" those.
                dst_joint_reader
                    .merge_version_vectors(dst_name)
                    .increment(*self.this_replica_id())
            }
            Ok(_) => return Err(Error::EntryExists),
            Err(e) => return Err(e),
        };

        drop(dst_joint_reader);
        drop(dst_joint_dir);

        let dst_dir = self.create_directory(dst_dir_path).await?;

        src_dir
            .move_entry(&src_name, &src_author, &dst_dir, dst_name, dst_vv)
            .await
    }

    /// Returns the local branch
    pub async fn local_branch(&self) -> Branch {
        self.shared.local_branch().await
    }

    /// Return the branch with the specified id.
    pub async fn branch(&self, id: &ReplicaId) -> Option<Branch> {
        self.shared.branch(id).await
    }

    /// Returns all branches
    pub async fn branches(&self) -> Vec<Branch> {
        self.shared.branches().await
    }

    /// Subscribe to change notification from all current and future branches.
    pub(crate) fn subscribe(&self) -> Subscription {
        self.shared.index.subscribe()
    }

    // Opens the root directory across all branches as JointDirectory.
    async fn joint_root(&self) -> Result<JointDirectory> {
        let branches = self.branches().await;
        let mut dirs = Vec::with_capacity(branches.len());

        for branch in branches {
            let dir = if branch.id() == self.this_replica_id() {
                branch.open_or_create_root().await.map_err(|error| {
                    log::error!(
                        "failed to open root directory on the local branch: {:?}",
                        error
                    );
                    error
                })?
            } else {
                match branch.open_root(self.local_branch().await).await {
                    Ok(dir) => dir,
                    Err(Error::EntryNotFound | Error::BlockNotFound(_)) => {
                        // Some branch roots may not have been loaded across the network yet. We'll
                        // ignore those.
                        continue;
                    }
                    Err(error) => {
                        log::error!(
                            "failed to open root directory on a remote branch {:?}: {:?}",
                            branch.id(),
                            error
                        );
                        return Err(error);
                    }
                }
            };

            dirs.push(dir);
        }

        Ok(JointDirectory::new(dirs).await)
    }

    pub(crate) fn index(&self) -> &Index {
        &self.shared.index
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        print.display(&"Repository");
        let branches = self.shared.branches.lock().await;
        for (replica_id, branch) in &*branches {
            let print = print.indent();
            let local = if replica_id == self.this_replica_id() {
                " (local)"
            } else {
                ""
            };
            print.display(&format_args!("Branch ID: {:?}{}", replica_id, local));
            let print = print.indent();
            print.display(&format_args!(
                "/, vv: {:?}",
                branch.data().root_version_vector().await
            ));
            branch.debug_print(print.indent()).await;
        }
    }
}

/// Opens or creates the repository database.
pub(crate) async fn open_db(store: db::Store) -> Result<db::Pool> {
    let pool = db::open(store).await?;

    block::init(&pool).await?;
    index::init(&pool).await?;

    Ok(pool)
}

struct Shared {
    index: Index,
    cryptor: Cryptor,
    // Cache for `Branch` instances to make them persistent over the lifetime of the program.
    branches: Mutex<HashMap<ReplicaId, Branch>>,
}

impl Shared {
    pub async fn local_branch(&self) -> Branch {
        self.inflate(self.index.branches().await.local()).await
    }

    pub async fn branch(&self, id: &ReplicaId) -> Option<Branch> {
        Some(self.inflate(self.index.branches().await.get(id)?).await)
    }

    pub async fn branches(&self) -> Vec<Branch> {
        future::join_all(
            self.index
                .branches()
                .await
                .all()
                .map(|data| self.inflate(data)),
        )
        .await
    }

    // Create `Branch` wrapping the given `data`, reusing a previously cached one if it exists,
    // and putting it into the cache it it does not.
    async fn inflate(&self, data: &Arc<BranchData>) -> Branch {
        self.branches
            .lock()
            .await
            .entry(*data.id())
            .or_insert_with(|| {
                Branch::new(self.index.pool.clone(), data.clone(), self.cryptor.clone())
            })
            .clone()
    }
}

// The merge algorithm.
struct Merger {
    shared: Arc<Shared>,
    tasks: FuturesUnordered<ScopedJoinHandle<ReplicaId>>,
    states: HashMap<ReplicaId, MergeState>,
}

impl Merger {
    fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            tasks: FuturesUnordered::new(),
            states: HashMap::new(),
        }
    }

    async fn run(mut self) {
        let mut rx = self.shared.index.subscribe();

        loop {
            select! {
                branch_id = rx.recv() => {
                    if let Some(branch_id) = branch_id {
                        self.handle_branch_changed(branch_id).await
                    } else {
                        break;
                    }
                }
                branch_id = self.tasks.next(), if !self.tasks.is_empty() => {
                    if let Some(Ok(branch_id)) = branch_id {
                        self.handle_task_finished(branch_id).await
                    }
                }
            }
        }
    }

    async fn handle_branch_changed(&mut self, branch_id: ReplicaId) {
        if branch_id == *self.shared.index.this_replica_id() {
            // local branch change - ignore.
            return;
        }

        if let Some(state) = self.states.get_mut(&branch_id) {
            // Merge of this branch is already ongoing - schedule to run it again after it's
            // finished.
            *state = MergeState::Pending;
            return;
        }

        self.spawn_task(branch_id).await
    }

    async fn handle_task_finished(&mut self, branch_id: ReplicaId) {
        if let Some(MergeState::Pending) = self.states.remove(&branch_id) {
            self.spawn_task(branch_id).await
        }
    }

    async fn spawn_task(&mut self, remote_id: ReplicaId) {
        let remote = if let Some(remote) = self.shared.branch(&remote_id).await {
            remote
        } else {
            // branch removed in the meantime - ignore.
            return;
        };

        let local = self.shared.local_branch().await;

        let handle = ScopedJoinHandle(task::spawn(async move {
            if *local.data().root_version_vector().await
                > *remote.data().root_version_vector().await
            {
                log::debug!(
                    "merge with branch {:?} suppressed - local branch already up to date",
                    remote_id
                );
                return remote_id;
            }

            log::debug!("merge with branch {:?} started", remote_id);

            match merge_branches(local, remote).await {
                Ok(()) => {
                    log::info!("merge with branch {:?} complete", remote_id)
                }
                Err(error) => {
                    // `EntryNotFound` most likely means the remote snapshot is not fully
                    // downloaded yet and `BlockNotFound` means that a block is not downloaded yet.
                    // Both error are harmless because the merge will be attempted again on the
                    // next change notification. We reduce the log severity for them to avoid log
                    // spam.
                    let level = if matches!(error, Error::EntryNotFound | Error::BlockNotFound(_)) {
                        Level::Trace
                    } else {
                        Level::Error
                    };

                    log::log!(
                        level,
                        "merge with branch {:?} failed: {}",
                        remote_id,
                        error.verbose()
                    )
                }
            }

            remote_id
        }));

        self.tasks.push(handle);
        self.states.insert(remote_id, MergeState::Ongoing);
    }
}

enum MergeState {
    // A merge is ongoing
    Ongoing,
    // A merge is ongoing and another one was already scheduled
    Pending,
}

async fn merge_branches(local: Branch, remote: Branch) -> Result<()> {
    let remote_root = remote.open_root(local.clone()).await?;
    JointDirectory::new(iter::once(remote_root))
        .await
        .merge()
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, index::RootNode};

    #[tokio::test(flavor = "multi_thread")]
    async fn root_directory_always_exists() {
        let replica_id = rand::random();
        let repo = Repository::open(db::Store::Memory, replica_id, Cryptor::Null, false)
            .await
            .unwrap();
        let _ = repo.open_directory("/").await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn merge() {
        let local_id = rand::random();
        let repo = Repository::open(db::Store::Memory, local_id, Cryptor::Null, true)
            .await
            .unwrap();

        // Add another branch to the index. Eventually there might be a more high-level API for
        // this but for now we have to resort to this.
        let remote_id = rand::random();
        let remote_node = RootNode::load_latest_or_create(&repo.index().pool, &remote_id)
            .await
            .unwrap();
        repo.index()
            .update_remote_branch(remote_id, remote_node)
            .await;

        let remote_branch = repo.branch(&remote_id).await.unwrap();
        let remote_root = remote_branch.open_or_create_root().await.unwrap();

        let local_branch = repo.local_branch().await;
        let local_root = local_branch.open_or_create_root().await.unwrap();

        let mut file = remote_root
            .create_file("test.txt".to_owned())
            .await
            .unwrap();
        file.write(b"hello").await.unwrap();
        file.flush().await.unwrap();

        let mut rx = local_branch.data().subscribe();

        loop {
            match local_root
                .read()
                .await
                .lookup_version("test.txt", &remote_id)
            {
                Ok(entry) => {
                    let content = entry
                        .file()
                        .unwrap()
                        .open()
                        .await
                        .unwrap()
                        .read_to_end()
                        .await
                        .unwrap();
                    assert_eq!(content, b"hello");
                    break;
                }
                Err(Error::EntryNotFound) => (),
                Err(error) => panic!("unexpected error: {:?}", error),
            }

            rx.changed().await.unwrap()
        }
    }
}
