use crate::error::Error;
use ouisync::{Registration, Repository, RepositoryId};
use serde::{Deserialize, Serialize};
use slab::Slab;
use std::{
    collections::{btree_map::Entry, BTreeMap},
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use thiserror::Error;

const EXPIRATION_KEY: &str = "expiration";

pub(crate) struct RepositorySet {
    repos: Slab<RepositoryHolder>,
    index: BTreeMap<PathBuf, usize>,
}

impl RepositorySet {
    pub fn new() -> Self {
        Self {
            repos: Slab::new(),
            index: BTreeMap::new(),
        }
    }

    pub fn try_insert(&mut self, holder: RepositoryHolder) -> Option<RepositoryHandle> {
        match self.index.entry(holder.path().to_owned()) {
            Entry::Vacant(entry) => {
                let handle = self.repos.insert(holder);
                entry.insert(handle);
                Some(RepositoryHandle(handle))
            }
            Entry::Occupied(_) => None,
        }
    }

    pub fn remove(&mut self, handle: RepositoryHandle) -> Option<RepositoryHolder> {
        let holder = self.repos.try_remove(handle.0)?;
        self.index.remove(holder.path());

        Some(holder)
    }

    pub fn get(&self, handle: RepositoryHandle) -> Option<&RepositoryHolder> {
        self.repos.get(handle.0)
    }

    pub fn get_mut(&mut self, handle: RepositoryHandle) -> Option<&mut RepositoryHolder> {
        self.repos.get_mut(handle.0)
    }

    pub fn find_by_path(&self, path: &Path) -> Option<(RepositoryHandle, &RepositoryHolder)> {
        let handle = self.index.get(path).copied()?;
        let holder = self.repos.get(handle)?;

        Some((RepositoryHandle(handle), holder))
    }

    pub fn find_by_id(&self, id: &RepositoryId) -> Option<(RepositoryHandle, &RepositoryHolder)> {
        // TODO: add repo_id -> repo_handle index to optimize this lookup
        self.repos
            .iter()
            .find(|(_, holder)| holder.repository().secrets().id() == id)
            .map(|(handle, holder)| (RepositoryHandle(handle), holder))
    }

    /// Finds repository whose path matches the given string. Succeeds only if exactly one
    /// repository matches.
    // TODO: consider returning all matches
    pub fn find_by_subpath(
        &self,
        pattern: &str,
    ) -> Result<(RepositoryHandle, &RepositoryHolder), FindError> {
        // TODO: This is very inefficient, although for small number of repos (which should be the
        // most common case) it's probably fine.
        let (handle, holder) = single(
            self.repos
                .iter()
                .filter(|(_, holder)| holder.path().to_string_lossy().contains(pattern)),
        )?;

        Ok((RepositoryHandle(handle), holder))
    }

    pub fn iter(&self) -> impl Iterator<Item = (RepositoryHandle, &RepositoryHolder)> {
        self.index.values().copied().filter_map(|handle| {
            self.repos
                .get(handle)
                .map(|holder| (RepositoryHandle(handle), holder))
        })
    }

    pub fn drain(&mut self) -> impl Iterator<Item = RepositoryHolder> + '_ {
        self.index.clear();
        self.repos.drain()
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Debug)]
#[serde(transparent)]
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
    registration: Option<Registration>,
}

impl RepositoryHolder {
    pub fn new(path: PathBuf, repo: Repository) -> Self {
        Self {
            path,
            repo: Arc::new(repo),
            registration: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Short name of the repository for information purposes. Might not be unique.
    pub fn short_name(&self) -> &str {
        self.path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
    }

    pub fn repository(&self) -> &Arc<Repository> {
        &self.repo
    }

    pub fn registration(&self) -> Option<&Registration> {
        self.registration.as_ref()
    }

    pub fn enable_sync(&mut self, registration: Registration) {
        self.registration = Some(registration);
    }

    pub fn disable_sync(&mut self) {
        self.registration = None;
    }

    pub async fn set_repository_expiration(&self, value: Option<Duration>) -> Result<(), Error> {
        if let Some(value) = value {
            self.repo
                .metadata()
                .set(
                    EXPIRATION_KEY,
                    value.as_millis().try_into().unwrap_or(u64::MAX),
                )
                .await?
        } else {
            self.repo.metadata().remove(EXPIRATION_KEY).await?
        }

        Ok(())
    }

    pub async fn repository_expiration(&self) -> Result<Option<Duration>, Error> {
        Ok(self
            .repo
            .metadata()
            .get(EXPIRATION_KEY)
            .await?
            .map(Duration::from_millis))
    }

    pub async fn close(&mut self) -> Result<(), Error> {
        self.registration = None;
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
