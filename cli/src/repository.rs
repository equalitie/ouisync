use crate::{options::Dirs, utils, DB_EXTENSION};
use camino::{Utf8Path, Utf8PathBuf};
use ouisync_bridge::{config::ConfigStore, error::Result};
use ouisync_lib::{
    network::{Network, Registration},
    Repository, StateMonitor,
};
use ouisync_vfs::MountGuard;
use std::{
    collections::HashMap,
    io, mem,
    sync::{Arc, Mutex, RwLock},
};
use tokio::{fs, runtime, task};
use tokio_stream::StreamExt;

// Config keys
pub(crate) const OPEN_ON_START: &str = "open_on_start";
pub(crate) const MOUNT_POINT: &str = "mount_point";

pub(crate) struct RepositoryHolder {
    pub repository: Arc<Repository>,
    pub registration: Registration,
    name: String,
    mount: Mutex<Option<Mount>>,
}

impl RepositoryHolder {
    pub async fn new(repository: Repository, name: String, network: &Network) -> Self {
        let repository = Arc::new(repository);
        let registration = network.register(repository.store().clone()).await;

        Self {
            repository,
            registration,
            name,
            mount: Mutex::new(None),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub async fn set_mount_point(&self, mount_point: Option<&str>) {
        let metadata = self.repository.metadata();

        if let Some(mount_point) = mount_point {
            metadata.set(MOUNT_POINT, mount_point).await.ok();
        } else {
            metadata.remove(MOUNT_POINT).await.ok();
        }
    }

    pub async fn mount(&self, mount_dir: &Utf8Path) -> Result<()> {
        let point: Option<String> = self.repository.metadata().get(MOUNT_POINT).await.ok();
        let point = point.map(|point| self.resolve_mount_point(point, mount_dir));

        let mount = if let Some(point) = point {
            let depth = match create_mount_point(&point).await {
                Ok(depth) => depth,
                Err(error) => {
                    tracing::error!(
                        name = self.name,
                        mount_point = %point,
                        ?error,
                        "failed to create mount point"
                    );

                    return Err(error.into());
                }
            };

            let guard = match ouisync_vfs::mount(
                runtime::Handle::current(),
                self.repository.clone(),
                point.clone(),
            ) {
                Ok(mount_guard) => {
                    tracing::info!(name = self.name, mount_point = %point, "repository mounted");
                    mount_guard
                }
                Err(error) => {
                    tracing::error!(
                        name = self.name,
                        mount_point = %point,
                        ?error,
                        "failed to mount repository"
                    );
                    return Err(error.into());
                }
            };

            Some(Mount {
                point,
                depth,
                guard,
            })
        } else {
            None
        };

        *self.mount.lock().unwrap() = mount;

        Ok(())
    }

    pub async fn unmount(&self) {
        let mount = self.mount.lock().unwrap().take();

        if let Some(Mount {
            point,
            depth,
            guard,
        }) = mount
        {
            // Make sure to unmount before attempting to remove the mount point.
            drop(guard);

            if let Err(error) = remove_mount_point(&point, depth).await {
                tracing::error!(name = self.name, mount_point = %point, ?error, "failed to remove mount point");
            }

            tracing::info!(name = self.name, mount_point = %point, "repository unmounted");
        }
    }

    pub fn is_mounted(&self) -> bool {
        self.mount.lock().unwrap().is_some()
    }

    fn resolve_mount_point(&self, mount_point: String, mount_dir: &Utf8Path) -> Utf8PathBuf {
        if mount_point.is_empty() {
            mount_dir.join(&self.name)
        } else {
            mount_point.into()
        }
    }
}

impl Drop for RepositoryHolder {
    fn drop(&mut self) {
        let repository = self.repository.clone();
        let name = self.name.clone();
        let mount = self
            .mount
            .lock()
            .unwrap()
            .take()
            .map(|mount| (mount.point, mount.depth));

        task::spawn(async move {
            if let Some((point, depth)) = mount {
                if let Err(error) = remove_mount_point(&point, depth).await {
                    tracing::error!(name, mount_point = %point, ?error, "failed to remove mount point");
                }
            }

            match repository.close().await {
                Ok(()) => {
                    tracing::info!(name, "repository closed");
                }
                Err(error) => {
                    tracing::error!(name, ?error, "failed to close repository");
                }
            }
        });
    }
}

struct Mount {
    point: Utf8PathBuf,
    // Number of trailing path components of `point` that were created by us. This is used to
    // delete only the directories we created on unmount.
    depth: u32,
    guard: MountGuard,
}

/// Create the mount point directory and returns the number of trailing path components that were
/// actually created. For example, if `path` is "/foo/bar/baz" and "/foo" already exists but not
/// "/foo/bar" then it returns 2 (because it created "/foo/bar" and "/foo/bar/baz").
async fn create_mount_point(path: &Utf8Path) -> io::Result<u32> {
    let depth = {
        let mut depth = 0;
        let mut path = path;

        loop {
            if fs::try_exists(path).await? {
                break;
            }

            if let Some(parent) = path.parent() {
                path = parent;
                depth += 1;
            } else {
                break;
            }
        }

        depth
    };

    fs::create_dir_all(path).await?;

    Ok(depth)
}

/// Remove the last `depth` components from `path`. For example, if `path` is "/foo/bar/baz" and
/// `depth` is 2, it removes "/foo/bar/baz" and then "/foo/bar" but not "/foo".
async fn remove_mount_point(path: &Utf8Path, depth: u32) -> io::Result<()> {
    let mut path = path;

    for _ in 0..depth {
        fs::remove_dir(path).await?;

        if let Some(parent) = path.parent() {
            path = parent;
        } else {
            break;
        }
    }

    Ok(())
}

#[derive(Default)]
pub(crate) struct RepositoryMap {
    inner: RwLock<HashMap<String, Arc<RepositoryHolder>>>,
}

impl RepositoryMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, holder: Arc<RepositoryHolder>) -> Option<Arc<RepositoryHolder>> {
        self.inner
            .write()
            .unwrap()
            .insert(holder.name.clone(), holder)
    }

    pub fn remove(&self, name: &str) -> Option<Arc<RepositoryHolder>> {
        self.inner.write().unwrap().remove(name)
    }

    pub fn remove_all(&self) -> Vec<Arc<RepositoryHolder>> {
        let inner = mem::take(&mut *self.inner.write().unwrap());
        inner.into_values().collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<RepositoryHolder>> {
        self.inner.read().unwrap().get(name).cloned()
    }

    pub fn get_all(&self) -> Vec<Arc<RepositoryHolder>> {
        self.inner.read().unwrap().values().cloned().collect()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.inner.read().unwrap().contains_key(name)
    }
}

// Find repositories that are marked to be opened on startup and open them.
pub(crate) async fn find_all(
    dirs: &Dirs,
    network: &Network,
    config: &ConfigStore,
    monitor: &StateMonitor,
) -> RepositoryMap {
    let mut walkdir = utils::walk_dir(&dirs.store_dir);
    let repositories = RepositoryMap::new();

    while let Some(entry) = walkdir.next().await {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::error!(%error, "failed to read directory entry");
                continue;
            }
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let path: &Utf8Path = match entry.path().try_into() {
            Ok(path) => path,
            Err(_) => {
                tracing::error!(path = ?entry.path(), "invalid repository path - not utf8");
                continue;
            }
        };

        if path.extension() != Some(DB_EXTENSION) {
            continue;
        }

        let repository =
            match ouisync_bridge::repository::open(path.to_path_buf(), None, config, monitor).await
            {
                Ok(repository) => repository,
                Err(error) => {
                    tracing::error!(?error, ?path, "failed to open repository");
                    continue;
                }
            };

        let metadata = repository.metadata();

        if !metadata.get(OPEN_ON_START).await.unwrap_or(false) {
            continue;
        }

        let name = path
            .strip_prefix(&dirs.store_dir)
            .unwrap_or(path)
            .with_extension("")
            .into_string();

        tracing::info!(name, "repository opened");

        let holder = RepositoryHolder::new(repository, name, network).await;
        let holder = Arc::new(holder);
        holder.mount(&dirs.mount_dir).await.ok();

        repositories.insert(holder);
    }

    repositories
}
