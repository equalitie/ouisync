use crate::{
    branch::{self, Branch},
    crypto::Cryptor,
    debug_printer::DebugPrinter,
    directory::{Directory, MoveDstDirectory},
    entry_type::EntryType,
    error::{Error, Result},
    file::File,
    index::{BranchData, Index, Subscription},
    joint_directory::JointDirectory,
    path,
    scoped_task::ScopedJoinHandle,
    ReplicaId,
};
use camino::Utf8Path;
use futures_util::{future, stream::FuturesUnordered, StreamExt};
use std::{collections::HashMap, sync::Arc};
use tokio::{select, sync::Mutex, task};

pub struct Repository {
    shared: Arc<Shared>,
    _merge_handle: ScopedJoinHandle<()>,
}

impl Repository {
    pub fn new(index: Index, cryptor: Cryptor) -> Self {
        let shared = Arc::new(Shared {
            index,
            cryptor,
            branches: Mutex::new(HashMap::new()),
        });

        let merge_handle = ScopedJoinHandle(task::spawn(Merger::new(shared.clone()).run()));

        Self {
            shared,
            _merge_handle: merge_handle,
        }
    }

    pub fn this_replica_id(&self) -> &ReplicaId {
        self.shared.index.this_replica_id()
    }

    /// Looks up an entry by its path. The path must be relative to the repository root.
    /// If the entry exists, returns its `EntryType`, otherwise returns `EntryNotFound`.
    pub async fn lookup_type<P: AsRef<Utf8Path>>(&self, path: P) -> Result<EntryType> {
        match path::decompose(path.as_ref()) {
            Some((parent, name)) => {
                let parent = self.open_directory(parent).await?;
                let parent = parent.read().await;
                Ok(parent.lookup_unique(name)?.entry_type())
            }
            None => Ok(EntryType::Directory),
        }
    }

    /// Opens a file at the given path (relative to the repository root)
    pub async fn open_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<File> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::EntryIsDirectory)?;

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
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::EntryIsDirectory)?;
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
    pub async fn create_file<P: AsRef<Utf8Path>>(&self, path: &P) -> Result<File> {
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
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::EntryIsDirectory)?;
        let dir = self.open_directory(parent).await?;
        dir.remove_file(self.this_replica_id(), name).await?;
        Ok(dir)
    }

    /// Removes the directory at the given path. The directory must be empty. Returns the parent
    /// directory.
    pub async fn remove_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<JointDirectory> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::OperationNotSupported)?;
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
        _src: S,
        _dst: D,
    ) -> Result<(Directory, MoveDstDirectory)> {
        todo!()
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
    pub fn subscribe(&self) -> Subscription {
        self.shared.index.subscribe()
    }

    // Opens the root directory across all branches as JointDirectory.
    async fn joint_root(&self) -> Result<JointDirectory> {
        let branches = self.branches().await;
        let mut dirs = Vec::with_capacity(branches.len());

        for branch in branches {
            let dir = if branch.id() == self.this_replica_id() {
                branch.open_or_create_root().await?
            } else {
                match branch.open_root(self.local_branch().await).await {
                    Ok(dir) => dir,
                    Err(Error::EntryNotFound) => {
                        // Some branch roots may not have been loaded across the network yet. We'll
                        // ignore those.
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            };

            dirs.push(dir);
        }

        Ok(JointDirectory::new(dirs).await)
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
            print.display(&format!("Branch {:?}{}", replica_id, local));
            print.display(&"/");
            branch.debug_print(print.indent()).await;
        }
    }
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
            if let Err(error) = branch::merge(local, remote).await {
                log::error!(
                    "failed to merge branch {:?}: {}",
                    remote_id,
                    error.verbose()
                );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test(flavor = "multi_thread")]
    async fn root_directory_always_exists() {
        let pool = db::init(db::Store::Memory).await.unwrap();
        let replica_id = rand::random();
        let index = Index::load(pool, replica_id).await.unwrap();
        let repo = Repository::new(index, Cryptor::Null);

        let _ = repo.open_directory("/").await.unwrap();
    }
}
