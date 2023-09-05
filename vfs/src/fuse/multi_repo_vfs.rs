use super::MountGuard;
use crate::{MountError, MultiRepoMount};
use ouisync_lib::Repository;
use std::{
    collections::HashMap,
    fs,
    future::{self, Future},
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::runtime::Handle as RuntimeHandle;

pub struct MultiRepoVFS {
    runtime_handle: RuntimeHandle,
    mount_point: PathBuf,
    repositories: Mutex<HashMap<PathBuf, MountGuard>>,
}

impl MultiRepoMount for MultiRepoVFS {
    fn create(
        mount_point: impl AsRef<Path>,
    ) -> Pin<Box<dyn Future<Output = Result<Self, MountError>> + Send>> {
        Box::pin(future::ready(Ok(Self {
            runtime_handle: RuntimeHandle::current(),
            mount_point: mount_point.as_ref().to_path_buf(),
            repositories: Mutex::new(HashMap::default()),
        })))
    }

    fn insert(&self, store_path: PathBuf, repo: Arc<Repository>) -> Result<(), io::Error> {
        let name = store_path.file_stem().ok_or_else(|| {
            io::Error::new(
                // InvalidFilename would have been better, but it's unstable.
                io::ErrorKind::InvalidInput,
                format!("invalid repository path: {:?}", store_path),
            )
        })?;

        let mount_point = self.mount_point.join(name);

        // TODO: this should be async
        fs::create_dir_all(&mount_point)?;

        let mount_guard = super::mount(self.runtime_handle.clone(), repo, mount_point)?;

        self.repositories
            .lock()
            .unwrap()
            .insert(store_path, mount_guard);

        Ok(())
    }

    fn remove(&self, store_path: &Path) -> Result<(), io::Error> {
        self.repositories.lock().unwrap().remove(store_path);
        Ok(())
    }
}
