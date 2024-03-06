use crate::{options::Dirs, utils, DB_EXTENSION};
use anyhow::{Context as _, Result};
use camino::Utf8Path;
use ouisync_bridge::{config::ConfigStore, protocol::remote::Request, transport::RemoteClient};
use ouisync_lib::{
    network::{Network, Registration},
    Repository,
};
use ouisync_vfs::MountGuard;
use state_monitor::StateMonitor;
use std::{
    borrow::{Borrow, Cow},
    collections::{btree_map::Entry, BTreeMap},
    ffi::OsStr,
    fmt, io, mem,
    ops::{Bound, Deref},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};
use thiserror::Error;
use tokio::{fs, runtime, task};
use tokio_stream::StreamExt;

// Config keys
pub(crate) const OPEN_ON_START: &str = "open_on_start";
pub(crate) const MOUNT_POINT: &str = "mount_point";

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub(crate) struct RepositoryName(Arc<str>);

impl RepositoryName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RepositoryName {
    type Error = InvalidRepositoryName;

    fn try_from(input: String) -> Result<Self, Self::Error> {
        if input.trim_start().starts_with('/') {
            return Err(InvalidRepositoryName);
        }

        if input.contains("..") {
            return Err(InvalidRepositoryName);
        }

        Ok(Self(input.into_boxed_str().into()))
    }
}

impl fmt::Display for RepositoryName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for RepositoryName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<Path> for RepositoryName {
    fn as_ref(&self) -> &Path {
        self.as_str().as_ref()
    }
}

impl Borrow<str> for RepositoryName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl Deref for RepositoryName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

#[derive(Debug, Error)]
#[error("invalid repository name")]
pub(crate) struct InvalidRepositoryName;

pub(crate) struct RepositoryHolder {
    pub repository: Arc<Repository>,
    pub registration: Registration,
    name: RepositoryName,
    mount: Mutex<Option<Mount>>,
}

impl RepositoryHolder {
    pub async fn new(repository: Repository, name: RepositoryName, network: &Network) -> Self {
        let repository = Arc::new(repository);
        let registration = network.register(repository.handle()).await;

        Self {
            repository,
            registration,
            name,
            mount: Mutex::new(None),
        }
    }

    pub fn name(&self) -> &RepositoryName {
        &self.name
    }

    // TODO: should `mount_point` be `Option<&Path>` ?
    pub async fn set_mount_point(&self, mount_point: Option<&str>) {
        let metadata = self.repository.metadata();

        if let Some(mount_point) = mount_point {
            metadata.set(MOUNT_POINT, mount_point).await.ok();
        } else {
            metadata.remove(MOUNT_POINT).await.ok();
        }
    }

    pub async fn mount(&self, mount_dir: &Path) -> Result<()> {
        let point: Option<String> = self
            .repository
            .metadata()
            .get(MOUNT_POINT)
            .await
            .ok()
            .flatten();
        let point = point.map(|point| self.resolve_mount_point(point, mount_dir));

        let mount = if let Some(point) = point {
            let depth = match create_mount_point(&point).await {
                Ok(depth) => depth,
                Err(error) => {
                    tracing::error!(
                        repo = %self.name,
                        mount_point = %point.display(),
                        ?error,
                        "Failed to create mount point"
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
                    tracing::info!(
                        repo = %self.name,
                        mount_point = %point.display(),
                        "Repository mounted"
                    );
                    mount_guard
                }
                Err(error) => {
                    tracing::error!(
                        repo = %self.name,
                        mount_point = %point.display(),
                        ?error,
                        "Failed to mount repository"
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
                tracing::error!(
                    repo = %self.name,
                    mount_point = %point.display(),
                    ?error,
                    "Failed to remove mount point"
                );
            }

            tracing::info!(
                repo = %self.name,
                mount_point = %point.display(),
                "Repository unmounted"
            );
        }
    }

    pub fn is_mounted(&self) -> bool {
        self.mount.lock().unwrap().is_some()
    }

    /// Create a mirror of the repository on the given remote host.
    pub async fn mirror(&self, host: &str, config: Arc<rustls::ClientConfig>) -> Result<()> {
        let secrets = self
            .repository
            .secrets()
            .into_write_secrets()
            .context("permission denied")?;

        let client = RemoteClient::connect(host, config).await.map_err(|error| {
            tracing::error!(?error, host, "connection failed");
            error
        })?;

        let proof = secrets.write_keys.sign(client.session_cookie().as_ref());
        let request = Request::Create {
            repository_id: secrets.id,
            proof,
        };

        client.invoke(request).await.map_err(|error| {
            tracing::error!(?error, host, "request failed");
            error
        })?;

        Ok(())
    }

    fn resolve_mount_point(&self, mount_point: String, mount_dir: &Path) -> PathBuf {
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
                    tracing::error!(
                        %name,
                        mount_point = %point.display(),
                        ?error,
                        "Failed to remove mount point"
                    );
                }
            }

            match repository.close().await {
                Ok(()) => {
                    tracing::info!(%name, "Repository closed");
                }
                Err(error) => {
                    tracing::error!(%name, ?error, "Failed to close repository");
                }
            }
        });
    }
}

struct Mount {
    point: PathBuf,
    // Number of trailing path components of `point` that were created by us. This is used to
    // delete only the directories we created on unmount.
    depth: u32,
    guard: MountGuard,
}

/// Create the mount point directory and returns the number of trailing path components that were
/// actually created. For example, if `path` is "/foo/bar/baz" and "/foo" already exists but not
/// "/foo/bar" then it returns 2 (because it created "/foo/bar" and "/foo/bar/baz").
async fn create_mount_point(path: &Path) -> io::Result<u32> {
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
async fn remove_mount_point(path: &Path, depth: u32) -> io::Result<()> {
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
    inner: RwLock<BTreeMap<RepositoryName, Arc<RepositoryHolder>>>,
}

impl RepositoryMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts the holder unless already exists. Returns whether the holder was inserted.
    pub fn try_insert(&self, holder: Arc<RepositoryHolder>) -> bool {
        match self.inner.write().unwrap().entry(holder.name.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(holder);
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    pub fn remove(&self, name: &str) -> Option<Arc<RepositoryHolder>> {
        self.inner.write().unwrap().remove(name)
    }

    pub fn remove_all(&self) -> Vec<Arc<RepositoryHolder>> {
        let inner = mem::take(&mut *self.inner.write().unwrap());
        inner.into_values().collect()
    }

    #[allow(unused)]
    pub fn get(&self, name: &str) -> Option<Arc<RepositoryHolder>> {
        self.inner.read().unwrap().get(name).cloned()
    }

    // Find entry whose name starts with the given prefix. Fails if no such entry exists or if more
    // than one such entry exists.
    pub fn find(&self, prefix: &str) -> Result<Arc<RepositoryHolder>, FindError> {
        let inner = self.inner.read().unwrap();

        let mut entries = inner
            .range::<str, _>((Bound::Included(prefix), Bound::Unbounded))
            .filter(|(key, _)| key.starts_with(prefix));

        let (_, holder) = entries.next().ok_or(FindError::NotFound)?;
        if entries.next().is_none() {
            Ok(holder.clone())
        } else {
            Err(FindError::Ambiguous)
        }
    }

    pub fn get_all(&self) -> Vec<Arc<RepositoryHolder>> {
        self.inner.read().unwrap().values().cloned().collect()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.inner.read().unwrap().contains_key(name)
    }
}

#[derive(Debug, Error)]
pub(crate) enum FindError {
    #[error("repository not found")]
    NotFound,
    #[error("repository name is ambiguous")]
    Ambiguous,
}

// Find repositories that are marked to be opened on startup and open them.
pub(crate) async fn find_all(
    dirs: &Dirs,
    network: &Network,
    config: &ConfigStore,
    monitor: &StateMonitor,
) -> RepositoryMap {
    let repositories = RepositoryMap::new();

    if !fs::try_exists(&dirs.store_dir).await.unwrap_or(false) {
        return repositories;
    }

    let mut walkdir = utils::walk_dir(&dirs.store_dir);

    while let Some(entry) = walkdir.next().await {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::error!(%error, "Failed to read directory entry");
                continue;
            }
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();

        if path.extension() != Some(OsStr::new(DB_EXTENSION)) {
            continue;
        }

        let repository =
            match ouisync_bridge::repository::open(path.to_path_buf(), None, config, monitor).await
            {
                Ok(repository) => repository,
                Err(error) => {
                    tracing::error!(?error, ?path, "Failed to open repository");
                    continue;
                }
            };

        let metadata = repository.metadata();

        if !metadata
            .get(OPEN_ON_START)
            .await
            .ok()
            .flatten()
            .unwrap_or(false)
        {
            continue;
        }

        let name = path
            .strip_prefix(&dirs.store_dir)
            .unwrap_or(path)
            .with_extension("")
            .to_string_lossy()
            .into_owned()
            .try_into()
            // This unwrap should be ok because RepositoryName is only not allowed to start with
            // "/" or contain "..", none of which can happen here.
            .unwrap();

        tracing::info!(%name, "Repository opened");

        let holder = RepositoryHolder::new(repository, name, network).await;
        let holder = Arc::new(holder);
        holder.mount(&dirs.mount_dir).await.ok();

        assert!(repositories.try_insert(holder));
    }

    repositories
}

pub(crate) async fn delete_store(store_dir: &Path, repository_name: &str) -> io::Result<()> {
    ouisync_lib::delete_repository(store_path(store_dir, repository_name)).await?;

    // Remove ancestors directories up to `store_dir` but only if they are empty.
    for dir in Utf8Path::new(repository_name).ancestors().skip(1) {
        let path = store_dir.join(dir);

        if path == store_dir {
            break;
        }

        // TODO: When `io::ErrorKind::DirectoryNotEmpty` is stabilized, we should break only on that
        // error and propagate the rest.
        if let Err(error) = fs::remove_dir(&path).await {
            tracing::error!(
                repo = repository_name,
                path = %path.display(),
                ?error,
                "Failed to remove repository store subdirectory"
            );
            break;
        }
    }

    Ok(())
}

pub(crate) fn store_path(store_dir: &Path, repository_name: &str) -> PathBuf {
    let suffix = Path::new(repository_name);
    let extension = if let Some(extension) = suffix.extension() {
        let mut extension = extension.to_owned();
        extension.push(".");
        extension.push(DB_EXTENSION);

        Cow::Owned(extension)
    } else {
        Cow::Borrowed(DB_EXTENSION.as_ref())
    };

    store_dir.join(suffix).with_extension(extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_path_sanity_check() {
        let store_dir = Path::new("/home/alice/ouisync/store");

        assert_eq!(
            store_path(store_dir, "foo"),
            Path::new("/home/alice/ouisync/store/foo.ouisyncdb")
        );

        assert_eq!(
            store_path(store_dir, "foo/bar"),
            Path::new("/home/alice/ouisync/store/foo/bar.ouisyncdb")
        );

        assert_eq!(
            store_path(store_dir, "foo/bar.baz"),
            Path::new("/home/alice/ouisync/store/foo/bar.baz.ouisyncdb")
        );
    }
}
