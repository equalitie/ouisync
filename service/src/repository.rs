use crate::error::Error;
use ouisync::{Registration, Repository, RepositoryId};
use ouisync_macros::api;
use serde::{Deserialize, Serialize};
use slab::Slab;
use std::{
    collections::{btree_map::Entry, BTreeMap},
    fmt, mem,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};
use thiserror::Error;

const EXPIRATION_KEY: &str = "expiration";

pub(crate) struct RepositorySet {
    inner: RwLock<Inner>,
}

struct Inner {
    repos: Slab<RepositoryHolder>,
    index: BTreeMap<PathBuf, usize>,
}

impl RepositorySet {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                repos: Slab::new(),
                index: BTreeMap::new(),
            }),
        }
    }

    pub fn try_insert(&self, holder: RepositoryHolder) -> Option<RepositoryHandle> {
        let mut inner = self.inner.write().unwrap();
        let inner = &mut *inner;

        match inner.index.entry(holder.path().to_owned()) {
            Entry::Vacant(entry) => {
                let handle = inner.repos.insert(holder);
                entry.insert(handle);
                Some(RepositoryHandle(handle))
            }
            Entry::Occupied(_) => None,
        }
    }

    pub fn replace(
        &self,
        handle: RepositoryHandle,
        holder: RepositoryHolder,
    ) -> Option<RepositoryHolder> {
        let mut inner = self.inner.write().unwrap();
        let inner = &mut *inner;

        let old_holder = inner.repos.get_mut(handle.0)?;

        match inner.index.entry(holder.path.clone()) {
            Entry::Vacant(entry) => entry.insert(handle.0),
            Entry::Occupied(_) => return None,
        };

        inner.index.remove(&old_holder.path);

        Some(mem::replace(old_holder, holder))
    }

    pub fn remove(&self, handle: RepositoryHandle) -> Option<RepositoryHolder> {
        let mut inner = self.inner.write().unwrap();

        let holder = inner.repos.try_remove(handle.0)?;
        inner.index.remove(holder.path());

        Some(holder)
    }

    /// Calls the given function with the repository holder corresponding to the given handle.
    pub fn with<F, R>(&self, handle: RepositoryHandle, f: F) -> Option<R>
    where
        F: FnOnce(&RepositoryHolder) -> R,
    {
        self.inner.read().unwrap().repos.get(handle.0).map(f)
    }

    pub fn find_by_path(&self, path: &Path) -> Option<RepositoryHandle> {
        self.inner
            .read()
            .unwrap()
            .index
            .get(path)
            .copied()
            .map(RepositoryHandle)
    }

    /// Finds repository whose path matches the given string. Succeeds only if exactly one
    /// repository matches.
    // TODO: consider returning all matches
    pub fn find_by_subpath(&self, pattern: &str) -> Result<RepositoryHandle, FindError> {
        // TODO: This is very inefficient, although for small number of repos (which should be the
        // most common case) it's probably fine.
        single(
            self.inner
                .read()
                .unwrap()
                .repos
                .iter()
                .filter(|(_, holder)| holder.path().to_string_lossy().contains(pattern))
                .map(|(handle, _)| RepositoryHandle(handle)),
        )
    }

    pub fn find_by_id(&self, id: &RepositoryId) -> Option<RepositoryHandle> {
        // TODO: add repo_id -> repo_handle index to optimize this lookup
        self.inner
            .read()
            .unwrap()
            .repos
            .iter()
            .find(|(_, holder)| holder.repository().secrets().id() == id)
            .map(|(handle, _)| RepositoryHandle(handle))
    }

    pub fn get_repository(&self, handle: RepositoryHandle) -> Option<Arc<Repository>> {
        self.with(handle, |holder| holder.repository().clone())
    }

    pub fn get_repository_and_short_name(
        &self,
        handle: RepositoryHandle,
    ) -> Option<(Arc<Repository>, String)> {
        self.with(handle, |holder| {
            (holder.repository().clone(), holder.short_name().to_owned())
        })
    }

    /// Maps all entries using the given function and collects the results.
    pub fn map<F, R, C>(&self, mut f: F) -> C
    where
        F: FnMut(RepositoryHandle, &RepositoryHolder) -> R,
        C: FromIterator<R>,
    {
        let inner = self.inner.read().unwrap();

        inner
            .index
            .values()
            .copied()
            .filter_map(|handle| {
                inner
                    .repos
                    .get(handle)
                    .map(|holder| f(RepositoryHandle(handle), holder))
            })
            .collect()
    }

    /// Removes and collects all entries.
    pub fn drain<C>(&self) -> C
    where
        C: FromIterator<RepositoryHolder>,
    {
        let mut inner = self.inner.write().unwrap();

        inner.index.clear();
        inner.repos.drain().collect()
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Debug)]
#[serde(transparent)]
#[api]
pub struct RepositoryHandle(usize);

impl RepositoryHandle {
    #[cfg(test)]
    pub(crate) fn from_raw(raw: usize) -> Self {
        Self(raw)
    }
}

impl fmt::Display for RepositoryHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub(crate) struct RepositoryHolder {
    path: PathBuf,
    repo: Arc<Repository>,
    registration: Mutex<Option<Registration>>,
}

impl RepositoryHolder {
    pub fn new(path: PathBuf, repo: Repository) -> Self {
        Self {
            path,
            repo: Arc::new(repo),
            registration: Mutex::new(None),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Short name of the repository for information purposes. Might not be unique.
    pub fn short_name(&self) -> &str {
        short_name(&self.path)
    }

    pub fn repository(&self) -> &Arc<Repository> {
        &self.repo
    }

    pub fn with_registration<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&Registration) -> R,
    {
        self.registration.lock().unwrap().as_ref().map(f)
    }

    pub fn is_sync_enabled(&self) -> bool {
        self.registration.lock().unwrap().is_some()
    }

    pub fn enable_sync(&self, registration: Registration) {
        *self.registration.lock().unwrap() = Some(registration);
    }

    pub fn disable_sync(&self) {
        *self.registration.lock().unwrap() = None;
    }

    pub async fn close(&self) -> Result<(), Error> {
        self.disable_sync();
        self.repo.close().await?;

        Ok(())
    }
}

#[derive(Error, Eq, PartialEq, Debug)]
pub(crate) enum FindError {
    #[error("repository not found")]
    NotFound,
    #[error("repository name is ambiguous")]
    Ambiguous,
}

pub(crate) async fn set_repository_expiration(
    repo: &Repository,
    value: Option<Duration>,
) -> Result<(), Error> {
    if let Some(value) = value {
        repo.metadata()
            .set(
                EXPIRATION_KEY,
                value.as_millis().try_into().unwrap_or(u64::MAX),
            )
            .await?
    } else {
        repo.metadata().remove(EXPIRATION_KEY).await?
    }

    Ok(())
}

pub(crate) async fn repository_expiration(repo: &Repository) -> Result<Option<Duration>, Error> {
    Ok(repo
        .metadata()
        .get(EXPIRATION_KEY)
        .await?
        .map(Duration::from_millis))
}

pub(crate) fn short_name(path: &Path) -> &str {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
}

fn single<T>(iter: T) -> Result<T::Item, FindError>
where
    T: IntoIterator,
{
    let mut iter = iter.into_iter();
    let item = iter.next().ok_or(FindError::NotFound)?;

    if iter.next().is_none() {
        Ok(item)
    } else {
        Err(FindError::Ambiguous)
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
// }
