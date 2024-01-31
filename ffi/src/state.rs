use crate::{
    file::FileHolder,
    registry::{Handle, InvalidHandle, Registry},
    repository::{RepositoryHandle, RepositoryHolder},
};
use deadlock::BlockingMutex;
use once_cell::sync::OnceCell;
use ouisync_bridge::{config::ConfigStore, transport};
use ouisync_lib::network::Network;
use ouisync_vfs::{MountError, MultiRepoMount, MultiRepoVFS};
use scoped_task::ScopedJoinHandle;
use state_monitor::StateMonitor;
use std::{collections::BTreeSet, future::Future, io, path::PathBuf, sync::Arc};
use tokio::sync::oneshot;

pub(crate) struct State {
    pub root_monitor: StateMonitor,
    pub repos_monitor: StateMonitor,
    pub config: ConfigStore,
    pub network: Network,
    repositories: Registry<Arc<RepositoryHolder>>,
    pub files: Registry<Arc<FileHolder>>,
    pub tasks: Registry<ScopedJoinHandle<()>>,
    pub cache_servers: BlockingMutex<BTreeSet<String>>,
    pub remote_client_config: OnceCell<Arc<rustls::ClientConfig>>,
    pub mounter: BlockingMutex<Option<MultiRepoVFS>>,
}

impl State {
    pub fn new(configs_path: PathBuf, root_monitor: StateMonitor) -> Self {
        let config = ConfigStore::new(configs_path);

        let network = Network::new(
            Some(config.dht_contacts_store()),
            root_monitor.make_child("Network"),
        );

        let repos_monitor = root_monitor.make_child("Repositories");

        Self {
            root_monitor,
            repos_monitor,
            config,
            network,
            repositories: Registry::new(),
            files: Registry::new(),
            tasks: Registry::new(),
            cache_servers: BlockingMutex::new(BTreeSet::new()),
            remote_client_config: OnceCell::new(),
            mounter: BlockingMutex::new(None),
        }
    }

    /// Cancel a notification subscription.
    pub fn unsubscribe(&self, handle: SubscriptionHandle) {
        self.tasks.remove(handle);
    }

    pub fn get_remote_client_config(&self) -> Result<Arc<rustls::ClientConfig>, io::Error> {
        self.remote_client_config
            .get_or_try_init(|| transport::make_client_config(&[]))
            .cloned()
    }

    pub fn insert_repository(&self, holder: RepositoryHolder) -> RepositoryHandle {
        if let Some(mounter) = &*self.mounter.lock().unwrap() {
            if let Err(error) = mounter.insert(holder.store_path.clone(), holder.repository.clone())
            {
                tracing::error!(
                    "Failed to mount repository {:?}: {error:?}",
                    holder.store_path
                );
            }
        }

        self.repositories.insert(Arc::new(holder))
    }

    pub fn remove_repository(&self, handle: RepositoryHandle) -> Option<Arc<RepositoryHolder>> {
        let holder = self.repositories.remove(handle)?;

        if let Some(mounter) = &*self.mounter.lock().unwrap() {
            if let Err(error) = mounter.remove(&holder.store_path) {
                tracing::error!(
                    "Failed to unmount repository {:?}: {error:?}",
                    holder.store_path
                );
            }
        }

        Some(holder)
    }

    pub fn get_repository(
        &self,
        handle: RepositoryHandle,
    ) -> Result<Arc<RepositoryHolder>, InvalidHandle> {
        self.repositories.get(handle)
    }

    pub fn collect_repository_handles(&self) -> Vec<RepositoryHandle> {
        self.repositories.collect_handles()
    }

    pub async fn mount_all(&self, mount_point: PathBuf) -> Result<(), MountError> {
        let mounter = MultiRepoVFS::create(mount_point).await.map_err(|error| {
            tracing::error!("Failed create mounter: {error:?}");
            error
        })?;

        for repo_holder in self.repositories.collect_values() {
            if let Err(error) = mounter.insert(
                repo_holder.store_path.clone(),
                repo_holder.repository.clone(),
            ) {
                tracing::error!(
                    "Failed to mount repository {:?}: {error:?}",
                    repo_holder.store_path
                );
            }
        }

        *self.mounter.lock().unwrap() = Some(mounter);

        Ok(())
    }

    /// Spawns a task and inserts it into the `tasks` registry. Returns its Registry handle.
    pub fn spawn_task<M, F>(&self, make_task: M) -> Handle<ScopedJoinHandle<()>>
    where
        M: FnOnce(u64) -> F + Send + 'static,
        F: Future<Output = ()> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();

        let task = scoped_task::spawn(async move {
            let Ok(id) = rx.await else {
                return;
            };

            make_task(id).await
        });

        let handle = self.tasks.insert(task);
        tx.send(handle.id()).ok();

        handle
    }
}

pub(crate) type SubscriptionHandle = Handle<ScopedJoinHandle<()>>;
