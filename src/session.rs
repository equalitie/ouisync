use crate::{
    config,
    crypto::Cryptor,
    db,
    error::{Error, Result},
    network::{Network, NetworkOptions},
    repository::{IndexMap, Repository, RepositoryManager},
};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::RwLock;

/// Entry point to this library.
pub struct Session {
    repositories: RepositoryManager,
    network: Network,
}

impl Session {
    /// Creates a new session.
    pub async fn new(
        config_db_store: db::Store,
        cryptor: Cryptor,
        network_options: NetworkOptions,
    ) -> Result<Self> {
        let pool = config::open_db(config_db_store).await?;

        let index_map = IndexMap::new(pool).await?;
        let index_map = Arc::new(RwLock::new(index_map));

        let repositories = RepositoryManager::new(index_map.clone(), cryptor);
        let network = Network::new(index_map, network_options)
            .await
            .map_err(Error::Network)?;

        Ok(Self {
            repositories,
            network,
        })
    }

    pub fn repositories(&self) -> &RepositoryManager {
        &self.repositories
    }

    /// Opens a repository.
    ///
    /// NOTE: Currently only one repository is supported but in the future this function will take
    /// an argument to specify which repository to open.
    pub fn open_repository(&self, enable_merger: bool) -> Repository {
        todo!()
        // Repository::new(self.index.clone(), self.cryptor.clone(), enable_merger)
    }

    /// Returns the local socket address the network listener is bound to.
    pub fn local_addr(&self) -> &SocketAddr {
        self.network.local_addr()
    }
}
