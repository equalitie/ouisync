use crate::{
    crypto::Cryptor,
    db,
    error::{Error, Result},
    index::Index,
    network::Network,
    repository::Repository,
    this_replica,
};

/// Entry point to this library.
pub struct Session {
    // TODO: cryptor should probably be per repository
    cryptor: Cryptor,
    index: Index,
    _network: Network,
}

impl Session {
    /// Creates a new session.
    pub async fn new(
        db_store: db::Store,
        cryptor: Cryptor,
        enable_local_discovery: bool,
    ) -> Result<Self> {
        let pool = db::init(db_store).await?;
        let this_replica_id = this_replica::get_or_create_id(&pool).await?;
        let index = Index::load(pool.clone(), this_replica_id).await?;
        let network = Network::new(enable_local_discovery, index.clone())
            .await
            .map_err(Error::Network)?;

        Ok(Self {
            cryptor,
            index,
            _network: network,
        })
    }

    /// Opens a repository.
    ///
    /// NOTE: Currently only one repository is supported but in the future this function will take
    /// an argument to specify which repository to open.
    pub fn open_repository(&self) -> Repository {
        Repository::new(self.index.clone(), self.cryptor.clone())
    }
}
