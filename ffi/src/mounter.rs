use crate::error::Error;
use ouisync_lib::Repository;
use ouisync_vfs::{MultiRepoMount, MultiRepoVFS};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub(crate) struct Mounter {
    inner: Mutex<Option<MultiRepoVFS>>,
}

impl Mounter {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    pub fn mount(&self, store_path: &Path, repository: &Arc<Repository>) -> Result<(), Error> {
        self.inner
            .lock()
            .unwrap()
            .as_ref()
            .map(|inner| inner.insert(store_path.to_owned(), repository.clone()))
            .unwrap_or(Ok(()))
            .map_err(|error| {
                tracing::error!("Failed to mount repository {:?}: {error:?}", store_path);
                error.into()
            })
    }

    pub fn unmount(&self, store_path: &Path) -> Result<(), Error> {
        self.inner
            .lock()
            .unwrap()
            .as_ref()
            .map(|inner| inner.remove(store_path))
            .unwrap_or(Ok(()))
            .map_err(|error| {
                tracing::error!("Failed to unmount repository {:?}: {error:?}", store_path);
                error.into()
            })
    }

    pub async fn mount_all(
        &self,
        mount_point: PathBuf,
        repos: impl IntoIterator<Item = (&Path, &Arc<Repository>)>,
    ) -> Result<(), Error> {
        let inner = MultiRepoVFS::create(mount_point).await.map_err(|error| {
            tracing::error!("Failed to create mounter: {error:?}");
            error
        })?;

        for (store_path, repo) in repos {
            if let Err(error) = inner.insert(store_path.to_owned(), repo.clone()) {
                tracing::error!("Failed to mount repository {:?}: {error:?}", store_path);
            }
        }

        *self.inner.lock().unwrap() = Some(inner);

        Ok(())
    }
}
