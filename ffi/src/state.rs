use crate::{
    file::FileHolder,
    registry::{Handle, Registry},
    repository::RepositoryHolder,
};
use once_cell::sync::OnceCell;
use ouisync_bridge::{config::ConfigStore, transport};
use ouisync_lib::{
    deadlock::{BlockingMutex, BlockingRwLockReadGuard},
    network::Network,
    StateMonitor,
};
use ouisync_vfs::MultiRepoVFS;
use scoped_task::ScopedJoinHandle;
use std::{
    collections::{BTreeSet, HashMap},
    io,
    path::PathBuf,
    sync::Arc,
};

pub(crate) struct State {
    pub root_monitor: StateMonitor,
    pub repos_monitor: StateMonitor,
    pub config: ConfigStore,
    pub network: Network,
    repositories: Registry<RepositoryHolder>,
    pub files: Registry<FileHolder>,
    pub tasks: Registry<ScopedJoinHandle<()>>,
    pub storage_servers: BlockingMutex<BTreeSet<String>>,
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
            storage_servers: BlockingMutex::new(BTreeSet::new()),
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

    pub fn insert_repository(&self, holder: RepositoryHolder) -> Handle<RepositoryHolder> {
        if let Some(mounter) = &*self.mounter.lock().unwrap() {
            if let Err(error) =
                mounter.add_repo(holder.store_path.clone(), holder.repository.clone())
            {
                tracing::error!(
                    "Failed to mount repository {:?}: {error:?}",
                    holder.store_path
                );
            }
        }

        self.repositories.insert(holder)
    }

    pub fn remove_repository(&self, handle: Handle<RepositoryHolder>) -> Option<RepositoryHolder> {
        if let Some(mounter) = &*self.mounter.lock().unwrap() {
            let repository = self.repositories.get(handle);
            mounter.remove_repo(repository.store_path.clone());
        }

        self.repositories.remove(handle)
    }

    pub fn get_repository(&self, handle: Handle<RepositoryHolder>) -> Arc<RepositoryHolder> {
        self.repositories.get(handle)
    }

    // Used on operating systems where `MultiRepoVFS` from `ouisync_vfs` is implemented.
    #[allow(dead_code)]
    pub fn read_repositories(
        &self,
    ) -> BlockingRwLockReadGuard<HashMap<u64, Arc<RepositoryHolder>>> {
        self.repositories.read()
    }
}

pub(crate) type SubscriptionHandle = Handle<ScopedJoinHandle<()>>;
