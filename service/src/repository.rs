use crate::{error::Error, protocol::Pattern};
use ouisync::{Registration, Repository};
use serde::{Deserialize, Serialize};
use slab::Slab;
use std::{
    collections::{btree_map::Entry, BTreeMap},
    fmt,
    ops::Bound,
    time::Duration,
};

const EXPIRATION_KEY: &str = "expiration";

pub(crate) struct RepositorySet {
    repos: Slab<RepositoryHolder>,
    index: BTreeMap<String, usize>,
}

impl RepositorySet {
    pub fn new() -> Self {
        Self {
            repos: Slab::new(),
            index: BTreeMap::new(),
        }
    }

    pub fn insert(
        &mut self,
        holder: RepositoryHolder,
    ) -> (RepositoryHandle, Option<RepositoryHolder>) {
        let name = holder.name.clone();
        let handle = self.repos.insert(holder);

        let old_holder = self
            .index
            .insert(name, handle)
            .and_then(|old_handle| self.repos.try_remove(old_handle));

        (RepositoryHandle(handle), old_holder)
    }

    pub fn try_insert(&mut self, holder: RepositoryHolder) -> Option<RepositoryHandle> {
        match self.index.entry(holder.name().to_owned()) {
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
        self.index.remove(holder.name());

        Some(holder)
    }

    pub fn get(&self, handle: RepositoryHandle) -> Option<&RepositoryHolder> {
        self.repos.get(handle.0)
    }

    pub fn get_mut(&mut self, handle: RepositoryHandle) -> Option<&mut RepositoryHolder> {
        self.repos.get_mut(handle.0)
    }

    pub fn find<'a>(&'a self, pattern: &'a Pattern) -> impl Iterator<Item = RepositoryHandle> + 'a {
        self.index
            .range::<str, _>((Bound::Included(pattern.term()), Bound::Unbounded))
            .take_while(move |(name, _)| match pattern {
                Pattern::Prefix(pattern) => name.starts_with(pattern),
                Pattern::Exact(pattern) => name.as_str() == pattern,
            })
            .map(|(_, handle)| RepositoryHandle(*handle))
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug)]
#[serde(transparent)]
pub struct RepositoryHandle(usize);

impl fmt::Display for RepositoryHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub(crate) struct RepositoryHolder {
    name: String,
    repo: Repository,
    registration: Option<Registration>,
}

impl RepositoryHolder {
    pub fn new(name: String, repo: Repository) -> Self {
        Self {
            name,
            repo,
            registration: None,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn repository(&self) -> &Repository {
        &self.repo
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

// #[cfg(test)]
// mod tests {
//     use super::*;
// }
