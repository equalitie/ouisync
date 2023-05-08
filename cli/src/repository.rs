use crate::{options::Dirs, transport::remote::RemoteClient, utils, DB_EXTENSION};
use camino::{Utf8Path, Utf8PathBuf};
use ouisync_bridge::{
    config::ConfigStore,
    error::{Error, Result},
    protocol::remote::{Request, Response},
};
use ouisync_lib::{
    network::{Network, Registration},
    AccessMode, Repository, StateMonitor,
};
use ouisync_vfs::MountGuard;
use std::{
    borrow::{Borrow, Cow},
    collections::{hash_map::Entry, HashMap},
    fmt, io, mem,
    ops::Deref,
    sync::{Arc, Mutex, RwLock},
};
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

impl AsRef<Utf8Path> for RepositoryName {
    fn as_ref(&self) -> &Utf8Path {
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

#[derive(Debug)]
pub(crate) struct InvalidRepositoryName;

impl From<InvalidRepositoryName> for Error {
    fn from(_: InvalidRepositoryName) -> Self {
        Self::InvalidArgument
    }
}

pub(crate) struct RepositoryHolder {
    pub repository: Arc<Repository>,
    pub registration: Registration,
    name: RepositoryName,
    mount: Mutex<Option<Mount>>,
}

impl RepositoryHolder {
    pub async fn new(repository: Repository, name: RepositoryName, network: &Network) -> Self {
        let repository = Arc::new(repository);
        let registration = network.register(repository.store().clone()).await;

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
                        name = %self.name,
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
                    tracing::info!(name = %self.name, mount_point = %point, "repository mounted");
                    mount_guard
                }
                Err(error) => {
                    tracing::error!(
                        name = %self.name,
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
                tracing::error!(name = %self.name, mount_point = %point, ?error, "failed to remove mount point");
            }

            tracing::info!(name = %self.name, mount_point = %point, "repository unmounted");
        }
    }

    pub fn is_mounted(&self) -> bool {
        self.mount.lock().unwrap().is_some()
    }

    /// Create a mirror of the repository on the given remote host.
    pub async fn mirror(&self, host: &str) -> Result<()> {
        let client = RemoteClient::connect(host).await?;
        let request = Request::Mirror {
            share_token: self
                .repository
                .secrets()
                .with_mode(AccessMode::Blind)
                .into(),
        };
        let response = client.invoke(request).await?;

        match response {
            Response::None => Ok(()),
        }
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
                    tracing::error!(%name, mount_point = %point, ?error, "failed to remove mount point");
                }
            }

            match repository.close().await {
                Ok(()) => {
                    tracing::info!(%name, "repository closed");
                }
                Err(error) => {
                    tracing::error!(%name, ?error, "failed to close repository");
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
    inner: RwLock<HashMap<RepositoryName, Arc<RepositoryHolder>>>,
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
            .into_string()
            .try_into()
            // This unwrap should be ok because RepositoryName is only not allowed to start with
            // "/" or contain "..", none of which can happen here.
            .unwrap();

        tracing::info!(%name, "repository opened");

        let holder = RepositoryHolder::new(repository, name, network).await;
        let holder = Arc::new(holder);
        holder.mount(&dirs.mount_dir).await.ok();

        assert!(repositories.try_insert(holder));
    }

    repositories
}

pub(crate) async fn delete_store(store_dir: &Utf8Path, repository_name: &str) -> io::Result<()> {
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
                name = repository_name,
                %path,
                ?error,
                "failed to remove repository store subdirectory"
            );
            break;
        }
    }

    Ok(())
}

pub(crate) fn store_path(store_dir: &Utf8Path, repository_name: &str) -> Utf8PathBuf {
    let suffix = Utf8Path::new(repository_name);
    let extension = if let Some(extension) = suffix.extension() {
        Cow::Owned(format!("{extension}.{DB_EXTENSION}"))
    } else {
        Cow::Borrowed(DB_EXTENSION)
    };

    store_dir.join(suffix).with_extension(extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_path_sanity_check() {
        let store_dir = Utf8Path::new("/home/alice/ouisync/store");

        assert_eq!(
            store_path(store_dir, "foo"),
            "/home/alice/ouisync/store/foo.ouisyncdb"
        );

        assert_eq!(
            store_path(store_dir, "foo/bar"),
            "/home/alice/ouisync/store/foo/bar.ouisyncdb"
        );

        assert_eq!(
            store_path(store_dir, "foo/bar.baz"),
            "/home/alice/ouisync/store/foo/bar.baz.ouisyncdb"
        );
    }
}
