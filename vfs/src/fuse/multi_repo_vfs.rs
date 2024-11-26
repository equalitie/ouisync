use super::MountGuard;
use crate::{MountError, MultiRepoMount};
use ouisync_lib::Repository;
use std::{
    collections::HashMap,
    fs,
    future::{self, Future},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::runtime::Handle as RuntimeHandle;

pub struct MultiRepoVFS {
    runtime_handle: RuntimeHandle,
    mount_root: PathBuf,
    repositories: Mutex<HashMap<String, Mount>>,
}

impl MultiRepoMount for MultiRepoVFS {
    fn create(
        mount_root: impl AsRef<Path>,
    ) -> impl Future<Output = Result<Self, MountError>> + Send {
        future::ready(Ok(Self {
            runtime_handle: RuntimeHandle::current(),
            mount_root: mount_root.as_ref().to_path_buf(),
            repositories: Mutex::new(HashMap::default()),
        }))
    }

    fn mount_root(&self) -> &Path {
        &self.mount_root
    }

    // TODO: make this idempotent (return Ok if already mounted)
    fn insert(&self, repo_name: String, repo: Arc<Repository>) -> Result<PathBuf, io::Error> {
        let mount_point = prepare_mountpoint(&repo_name, &self.mount_root)?;
        let mount_guard = super::mount(self.runtime_handle.clone(), repo, &mount_point)?;

        let mount = Mount {
            point: mount_point.clone(),
            guard: Some(mount_guard),
        };

        self.repositories.lock().unwrap().insert(repo_name, mount);

        Ok(mount_point)
    }

    fn remove(&self, repo_name: &str) -> Result<(), io::Error> {
        self.repositories.lock().unwrap().remove(repo_name);
        Ok(())
    }

    fn mount_point(&self, repo_name: &str) -> Option<PathBuf> {
        Some(
            self.repositories
                .lock()
                .unwrap()
                .get(repo_name)?
                .point
                .clone(),
        )
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

// TODO: should this be async?
fn prepare_mountpoint(repo_name: &str, mount_root: &Path) -> Result<PathBuf, io::Error> {
    let mount_point = mount_root.join(repo_name);

    let create_dir_error = match fs::create_dir_all(&mount_point) {
        Ok(()) => return Ok(mount_point),
        Err(error) => error,
    };

    if create_dir_error.kind() != io::ErrorKind::AlreadyExists {
        return Err(create_dir_error);
    }

    // At this point the `mount_point` exists, now check if we can use it.

    let mut read_dir = match mount_point.read_dir() {
        Ok(read_dir) => read_dir,
        Err(read_dir_error) => {
            if read_dir_error.kind() == io::ErrorKind::NotConnected {
                // Most likely a previous Ouisync process did not exit cleanly or otherwise failed
                // to unmount repositories. So let's try to unmount it first.  One disadvantage of
                // doing this is that we could accidentally unmount user's directories. However
                // MultiRepoVFS mounts everything under `mount_root` which is a dedicated to
                // Ouisync, so in practice this shouldn't be a problem.

                tracing::warn!(
                    "Mount point {mount_point:?} is not connected, attempting to unmount it"
                );

                std::process::Command::new("fusermount")
                    .arg("-u")
                    .arg(&mount_point)
                    .output()?;

                mount_point.read_dir()?
            } else {
                return Err(read_dir_error);
            }
        }
    };

    let dir_is_empty = read_dir.next().is_none();

    if !dir_is_empty {
        return Err(io::Error::new(
            // TODO: io::ErrorKind::DirectoryNotEmpty would have been better, but it's unstable
            io::ErrorKind::InvalidInput,
            format!("Mount directory {mount_point:?} is not empty"),
        ));
    }

    Ok(mount_point)
}
