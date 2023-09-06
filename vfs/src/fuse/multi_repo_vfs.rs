use super::MountGuard;
use crate::{MountError, MultiRepoMount};
use ouisync_lib::Repository;
use std::{
    collections::HashMap,
    ffi::OsStr,
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
    repositories: Mutex<HashMap<PathBuf, Mount>>,
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
        let mount_point = extract_mount_point(&store_path)?;
        let mount_point = self.mount_point.join(mount_point);

        // TODO: should this be async?
        fs::create_dir_all(&mount_point)?;

        let mount_guard = super::mount(self.runtime_handle.clone(), repo, &mount_point)?;

        let mount = Mount {
            point: mount_point,
            guard: Some(mount_guard),
        };

        self.repositories.lock().unwrap().insert(store_path, mount);

        Ok(())
    }

    fn remove(&self, store_path: &Path) -> Result<(), io::Error> {
        self.repositories.lock().unwrap().remove(store_path);
        Ok(())
    }
}

// Wrapper for `MountGuard` which also removes the mount directory after unmount on drop.
struct Mount {
    point: PathBuf,
    guard: Option<MountGuard>,
}

impl Drop for Mount {
    fn drop(&mut self) {
        self.guard.take();

        if let Err(error) = fs::remove_dir(&self.point) {
            tracing::error!(?error, path = ?self.point, "Failed to remove mount point");
        }
    }
}

fn extract_mount_point(store_path: &Path) -> Result<&OsStr, io::Error> {
    store_path.file_stem().ok_or_else(|| {
        io::Error::new(
            // InvalidFilename would have been better, but it's unstable.
            io::ErrorKind::InvalidInput,
            format!("invalid repository path: {:?}", store_path),
        )
    })
}
