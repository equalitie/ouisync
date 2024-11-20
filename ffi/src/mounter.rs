use crate::error::{Error, ErrorCode};
use ouisync_lib::Repository;
use ouisync_vfs::{MultiRepoMount, MultiRepoVFS};
use std::{
    collections::{hash_map::Entry, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

struct MounterInner {
    // Repositories may be `mount`ed or `unmount`ed before, after or during the `mount_root` call,
    // this hash map records what the user requested to be mounted or unmounted and applies the
    // operations once `mount_root` finishes mounting the root.
    repos: HashMap<PathBuf, Arc<Repository>>,
    multi_repo_vfs: Option<MultiRepoVFS>,
}

pub(crate) struct Mounter {
    inner: Mutex<MounterInner>,
}

impl Mounter {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(MounterInner {
                repos: Default::default(),
                multi_repo_vfs: None,
            }),
        }
    }

    pub fn mount(&self, store_path: &Path, repository: &Arc<Repository>) -> Result<(), Error> {
        let mut inner = self.inner.lock().unwrap();

        match inner.repos.entry(store_path.to_owned()) {
            Entry::Vacant(entry) => entry.insert(repository.clone()),
            Entry::Occupied(_) => {
                // We could also probably just ignore this error because `store_path` can't point
                // to more than one repository (so ignoring the error would make this function
                // idempotent).
                return Err(Error {
                    code: ErrorCode::EntryExists,
                    message: "The repository is already mounted".to_string(),
                });
            }
        };

        let result = inner
            .multi_repo_vfs
            .as_ref()
            .map(|vfs| {
                vfs.insert(store_path.to_owned(), repository.clone())
                    .map(|_| ())
            })
            .unwrap_or(Ok(()))
            .map_err(|error| {
                tracing::error!("Failed to mount repository {:?}: {error:?}", store_path);
                error.into()
            });

        if result.is_err() {
            inner.repos.remove(store_path);
        }

        result
    }

    pub fn unmount(&self, store_path: &Path) -> Result<(), Error> {
        let mut inner = self.inner.lock().unwrap();

        let result = inner
            .multi_repo_vfs
            .as_ref()
            .map(|inner| inner.remove(store_path))
            .unwrap_or(Ok(()))
            .map_err(|error| {
                tracing::error!("Failed to unmount repository {:?}: {error:?}", store_path);
                error.into()
            });

        if result.is_ok() {
            inner.repos.remove(store_path);
        }

        result
    }

    pub async fn mount_root(&self, mount_point: PathBuf) -> Result<(), Error> {
        let vfs = MultiRepoVFS::create(mount_point).await.map_err(|error| {
            tracing::error!("Failed to create mounter: {error:?}");
            error
        })?;

        let mut inner = self.inner.lock().unwrap();

        for (store_path, repo) in &inner.repos {
            if let Err(error) = vfs.insert(store_path.to_owned(), repo.clone()) {
                tracing::error!("Failed to mount repository {:?}: {error:?}", store_path);
            }
        }

        inner.multi_repo_vfs = Some(vfs);

        Ok(())
    }
}
