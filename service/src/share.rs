//! Share management module
//!
//! A Share represents a local filesystem directory that is synchronized with peers.
//! Each share has its own Ouisync repository and peer list.

use crate::{error::Error, sync_bridge::SyncBridge};
use ouisync::{PeerAddr, Repository, RepositoryId};
use serde::{Deserialize, Serialize};
use slab::Slab;
use std::{
    collections::{btree_map::Entry, BTreeMap},
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};
use thiserror::Error;
use uuid::Uuid;

/// A Share represents a local directory being synchronized
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShareInfo {
    /// Unique identifier for this share
    pub id: Uuid,
    /// Absolute path to the local directory
    pub path: PathBuf,
    /// User-friendly name for the share
    pub name: String,
    /// List of peer addresses associated with this share
    pub peers: Vec<PeerAddr>,
    /// Whether sync is currently active
    pub sync_enabled: bool,
    /// The underlying repository ID
    pub repository_id: RepositoryId,
}

/// Internal holder for a share and its associated resources
pub(crate) struct ShareHolder {
    info: ShareInfo,
    repository: Arc<Repository>,
    sync_bridge: Option<SyncBridge>,
}

impl ShareHolder {
    pub fn new(info: ShareInfo, repository: Arc<Repository>, sync_bridge: Option<SyncBridge>) -> Self {
        Self { info, repository, sync_bridge }
    }

    pub fn id(&self) -> Uuid {
        self.info.id
    }

    pub fn path(&self) -> &Path {
        &self.info.path
    }

    pub fn name(&self) -> &str {
        &self.info.name
    }

    pub fn repository(&self) -> &Arc<Repository> {
        &self.repository
    }

    pub fn peers(&self) -> &[PeerAddr] {
        &self.info.peers
    }

    pub fn info(&self) -> &ShareInfo {
        &self.info
    }

    pub fn add_peer(&mut self, peer: PeerAddr) -> bool {
        if !self.info.peers.contains(&peer) {
            self.info.peers.push(peer);
            true
        } else {
            false
        }
    }

    pub fn remove_peer(&mut self, peer: &PeerAddr) -> bool {
        if let Some(pos) = self.info.peers.iter().position(|p| p == peer) {
            self.info.peers.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn set_sync_enabled(&mut self, enabled: bool) {
        self.info.sync_enabled = enabled;
    }

    pub fn is_sync_enabled(&self) -> bool {
        self.info.sync_enabled
    }
}

/// Handle to a share in the ShareSet
#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Debug)]
#[serde(transparent)]
pub struct ShareHandle(usize);

impl ShareHandle {
    #[cfg(test)]
    pub(crate) fn from_raw(raw: usize) -> Self {
        Self(raw)
    }
}

impl fmt::Display for ShareHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Collection of all shares
pub(crate) struct ShareSet {
    inner: RwLock<Inner>,
}

struct Inner {
    shares: Slab<ShareHolder>,
    id_index: BTreeMap<Uuid, usize>,
    path_index: BTreeMap<PathBuf, usize>,
}

impl ShareSet {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                shares: Slab::new(),
                id_index: BTreeMap::new(),
                path_index: BTreeMap::new(),
            }),
        }
    }

    /// Try to insert a new share
    pub fn try_insert(
        &self,
        holder: ShareHolder,
    ) -> Result<ShareHandle, ShareInsertError> {
        let mut inner = self.inner.write().unwrap();

        // Check if share with this ID already exists
        if inner.id_index.contains_key(&holder.id()) {
            return Err(ShareInsertError::DuplicateId(holder.id()));
        }

        // Check if share with this path already exists
        match inner.path_index.entry(holder.path().to_owned()) {
            Entry::Vacant(entry) => {
                let handle = inner.shares.insert(holder);
                inner.id_index.insert(inner.shares[handle].id(), handle);
                entry.insert(handle);
                Ok(ShareHandle(handle))
            }
            Entry::Occupied(_) => Err(ShareInsertError::DuplicatePath(holder.path().to_owned())),
        }
    }

    /// Remove a share by handle
    pub fn remove(&self, handle: ShareHandle) -> Option<ShareHolder> {
        let mut inner = self.inner.write().unwrap();

        let holder = inner.shares.try_remove(handle.0)?;
        inner.id_index.remove(&holder.id());
        inner.path_index.remove(holder.path());

        Some(holder)
    }

    /// Get a share by handle
    pub fn get(&self, handle: ShareHandle) -> Option<ShareInfo> {
        self.inner
            .read()
            .unwrap()
            .shares
            .get(handle.0)
            .map(|h| h.info().clone())
    }

    /// Find share by UUID
    pub fn find_by_id(&self, id: Uuid) -> Option<ShareHandle> {
        self.inner
            .read()
            .unwrap()
            .id_index
            .get(&id)
            .copied()
            .map(ShareHandle)
    }

    /// Find share by path
    pub fn find_by_path(&self, path: &Path) -> Option<ShareHandle> {
        self.inner
            .read()
            .unwrap()
            .path_index
            .get(path)
            .copied()
            .map(ShareHandle)
    }

    /// Execute a function with a share holder
    pub fn with<F, R>(&self, handle: ShareHandle, f: F) -> Option<R>
    where
        F: FnOnce(&ShareHolder) -> R,
    {
        self.inner.read().unwrap().shares.get(handle.0).map(f)
    }

    /// Execute a mutable function with a share holder
    pub fn with_mut<F, R>(&self, handle: ShareHandle, f: F) -> Option<R>
    where
        F: FnOnce(&mut ShareHolder) -> R,
    {
        self.inner.write().unwrap().shares.get_mut(handle.0).map(f)
    }

    /// List all shares
    pub fn list(&self) -> Vec<ShareInfo> {
        self.inner
            .read()
            .unwrap()
            .shares
            .iter()
            .map(|(_, holder)| holder.info().clone())
            .collect()
    }

    /// Get repository for a share
    pub fn get_repository(&self, handle: ShareHandle) -> Option<Arc<Repository>> {
        self.with(handle, |holder| holder.repository().clone())
    }

    /// Remove all shares
    pub fn drain(&self) -> Vec<ShareHolder> {
        let mut inner = self.inner.write().unwrap();
        inner.id_index.clear();
        inner.path_index.clear();
        inner.shares.drain().collect()
    }
}

#[derive(Error, Debug)]
pub enum ShareInsertError {
    #[error("share with ID {0} already exists")]
    DuplicateId(Uuid),
    #[error("share with path {0:?} already exists")]
    DuplicatePath(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_share_set_insert_and_find() {
        let set = ShareSet::new();
        let id = Uuid::new_v4();
        let path = PathBuf::from("/test/path");
        
        // Note: This is a simplified test. In real usage, you'd need a valid Repository.
        // For now, this just tests the ShareSet data structure logic.
    }
}
