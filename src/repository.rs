use crate::{
    async_debug::{AsyncDebug, Printer},
    branch::Branch,
    crypto::Cryptor,
    directory::{Directory, MoveDstDirectory},
    entry_type::EntryType,
    error::{Error, Result},
    file::File,
    index::{BranchData, Index, Subscription},
    joint_directory::JointDirectory,
    path, ReplicaId,
};
use async_trait::async_trait;
use camino::Utf8Path;
use futures_util::future;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

pub struct Repository {
    index: Index,
    cryptor: Cryptor,
    // Cache for `Branch` instances to make them persistent over the lifetime of the program.
    branches: Mutex<HashMap<ReplicaId, Branch>>,
}

impl Repository {
    pub fn new(index: Index, cryptor: Cryptor) -> Self {
        Self {
            index,
            cryptor,
            branches: Mutex::new(HashMap::new()),
        }
    }

    pub fn this_replica_id(&self) -> &ReplicaId {
        self.index.this_replica_id()
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
        self.inflate(self.index.branches().await.local()).await
    }

    /// Return the branch with the specified id.
    pub async fn branch(&self, id: &ReplicaId) -> Option<Branch> {
        Some(self.inflate(self.index.branches().await.get(id)?).await)
    }

    /// Returns all branches
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

    /// Subscribe to change notification from all current and future branches.
    pub async fn subscribe(&self) -> Subscription {
        self.index.subscribe().await
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
}

#[async_trait]
impl AsyncDebug for Repository {
    async fn print(&self, print: &Printer) {
        print.string("Repository");
        let branches = self.branches.lock().await;
        for (replica_id, branch) in &*branches {
            let print = print.indent();
            let local = if replica_id == self.this_replica_id() {
                " (local)"
            } else {
                ""
            };
            print.string(&format!("Branch {:?}{}", replica_id, local));
            print.string("/");
            branch.print(&print.indent()).await;
        }
    }
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
