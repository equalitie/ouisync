/*
use crate::{error::Error, options::Dirs, utils, DB_EXTENSION};
use camino::Utf8Path;
use ouisync_bridge::{config::ConfigStore, protocol::remote::v1, transport::RemoteClient};
use ouisync_lib::{Network, Registration, Repository};
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
    time::Duration,
};
use thiserror::Error;
use tokio::{fs, runtime};
use tokio_rustls::rustls;
use tokio_stream::StreamExt;

// Config keys
pub(crate) const OPEN_ON_START: &str = "open_on_start";
const MOUNT_POINT_KEY: &str = "mount_point";

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
            metadata.set(MOUNT_POINT_KEY, mount_point).await.ok();
        } else {
            metadata.remove(MOUNT_POINT_KEY).await.ok();
        }
    }

    pub async fn mount(&self, mount_dir: &Path) -> io::Result<()> {
        let point: Option<String> = self
            .repository
            .metadata()
            .get(MOUNT_POINT_KEY)
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

                    return Err(error);
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
                    return Err(error);
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
        let Some(Mount {
            point,
            depth,
            guard,
        }) = self.mount.lock().unwrap().take()
        else {
            return;
        };

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

    pub fn is_mounted(&self) -> bool {
        self.mount.lock().unwrap().is_some()
    }

    /// Create a mirror of the repository on the given remote host.
    pub async fn mirror(&self, host: &str, config: Arc<rustls::ClientConfig>) -> Result<(), Error> {
        let secrets = self
            .repository
            .secrets()
            .into_write_secrets()
            .ok_or_else(|| Error::PermissionDenied)?;

        let client = RemoteClient::connect(host, config)
            .await
            .inspect_err(|error| tracing::error!(?error, host, "connection failed"))?;

        let proof = secrets.write_keys.sign(client.session_cookie().as_ref());
        let request = v1::Request::Create {
            repository_id: secrets.id,
            proof,
        };

        client
            .invoke(request)
            .await
            .inspect_err(|error| tracing::error!(?error, host, "request failed"))?;

        Ok(())
    }

    pub async fn close(&self) -> Result<(), Error> {
        self.unmount().await;

        self.repository
            .close()
            .await
            .inspect(|_| tracing::info!(name = %self.name, "Repository closed"))
            .inspect_err(
                |error| tracing::error!(name = %self.name, ?error, "Failed to close repository"),
            )?;

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
*/
