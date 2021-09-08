use super::{inner::Inner, parent_context::ParentContext, Directory};
use crate::{
    branch::Branch,
    error::{Error, Result},
    locator::Locator,
};
use std::{
    collections::{hash_map, HashMap},
    sync::{Arc, Weak},
};
use tokio::sync::{Mutex, RwLock};

// Cache for open root directory
// TODO: consider using the `ArcSwap` crate here.
pub(crate) struct RootDirectoryCache(Mutex<Option<Weak<RwLock<Inner>>>>);

impl RootDirectoryCache {
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }

    pub async fn open(&self, owner_branch: Branch, local_branch: Branch) -> Result<Directory> {
        let mut slot = self.0.lock().await;

        if let Some(inner) = slot.as_mut().and_then(|inner| inner.upgrade()) {
            Ok(Directory {
                inner,
                local_branch,
                parent: None,
            })
        } else {
            let dir = Directory::open_root(owner_branch, local_branch).await?;
            *slot = Some(Arc::downgrade(&dir.inner));
            Ok(dir)
        }
    }

    pub async fn open_or_create(&self, branch: Branch) -> Result<Directory> {
        let mut slot = self.0.lock().await;

        if let Some(inner) = slot.as_mut().and_then(|inner| inner.upgrade()) {
            Ok(Directory {
                inner,
                local_branch: branch,
                parent: None,
            })
        } else {
            let dir = Directory::open_or_create_root(branch).await?;
            *slot = Some(Arc::downgrade(&dir.inner));
            Ok(dir)
        }
    }
}

// Cache of open subdirectories.
pub(super) struct SubdirectoryCache(Mutex<HashMap<Locator, Weak<RwLock<Inner>>>>);

impl SubdirectoryCache {
    pub fn new() -> Self {
        Self(Mutex::new(HashMap::new()))
    }

    pub async fn open(
        &self,
        owner_branch: Branch,
        local_branch: Branch,
        locator: Locator,
        parent: ParentContext,
    ) -> Result<Directory> {
        let mut map = self.0.lock().await;

        let dir = match map.entry(locator) {
            hash_map::Entry::Occupied(mut entry) => {
                if let Some(inner) = entry.get().upgrade() {
                    Directory {
                        inner,
                        local_branch,
                        parent: Some(Box::new(parent)),
                    }
                } else {
                    let dir =
                        Directory::open(owner_branch, local_branch, locator, Some(parent)).await?;
                    entry.insert(Arc::downgrade(&dir.inner));
                    dir
                }
            }
            hash_map::Entry::Vacant(entry) => {
                let dir =
                    Directory::open(owner_branch, local_branch, locator, Some(parent)).await?;
                entry.insert(Arc::downgrade(&dir.inner));
                dir
            }
        };

        // Cleanup dead entries.
        map.retain(|_, dir| dir.upgrade().is_some());

        Ok(dir)
    }

    pub async fn create(
        &self,
        branch: Branch,
        locator: Locator,
        parent: ParentContext,
    ) -> Result<Directory> {
        let mut map = self.0.lock().await;

        let dir = match map.entry(locator) {
            hash_map::Entry::Occupied(_) => return Err(Error::EntryExists),
            hash_map::Entry::Vacant(entry) => {
                let dir = Directory::create(branch, locator, Some(parent));
                entry.insert(Arc::downgrade(&dir.inner));
                dir
            }
        };

        map.retain(|_, dir| dir.upgrade().is_some());

        Ok(dir)
    }
}
