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
    mem,
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
            if let Err(error) = fs::create_dir_all(&point).await {
                tracing::error!(
                    name = self.name,
                    mount_point = %point,
                    ?error,
                    "failed to create mount point"
                );
                return Err(error.into());
            }

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
                _guard: guard,
            })
        } else {
            None
        };

        *self.mount.lock().unwrap() = mount;

        Ok(())
    }

    pub async fn unmount(&self) {
        let mount = self.mount.lock().unwrap().take();

        if let Some(Mount { point, .. }) = mount {
            remove_mount_point(&point, &self.name).await;
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
        let mount_point = self.mount.lock().unwrap().take().map(|mount| mount.point);

        task::spawn(async move {
            if let Some(mount_point) = mount_point {
                remove_mount_point(&mount_point, &name).await;
            }

            match repository.close().await {
                Ok(()) => {
                    tracing::info!(?name, "repository closed");
                }
                Err(error) => {
                    tracing::error!(?name, ?error, "failed to close repository");
                }
            }
        });
    }
}

struct Mount {
    point: Utf8PathBuf,
    _guard: MountGuard,
}

async fn remove_mount_point(_mount_point: &Utf8Path, _repository_name: &str) {
    // TODO
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
