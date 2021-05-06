use crate::{
    branch::Branch,
    crypto::Cryptor,
    db,
    directory::Directory,
    entry::{Entry, EntryType},
    error::{Error, Result},
    file::File,
    locator::Locator,
    this_replica,
};

use std::sync::Arc;

pub struct Repository {
    pool: db::Pool,
    branch: Arc<Branch>,
    cryptor: Cryptor,
}

impl Repository {
    pub async fn new(pool: db::Pool, cryptor: Cryptor) -> Result<Self> {
        let replica_id = this_replica::get_or_create_id(&pool).await?;
        let branch = Arc::new(Branch::new(pool.clone(), replica_id).await?);

        Ok(Self {
            pool,
            branch,
            cryptor,
        })
    }

    /// Open an entry (file or directory).
    pub async fn open_entry(&self, locator: Locator, entry_type: EntryType) -> Result<Entry> {
        match entry_type {
            EntryType::File => Ok(Entry::File(self.open_file(locator).await?)),
            EntryType::Directory => Ok(Entry::Directory(self.open_directory(locator).await?)),
        }
    }

    pub async fn open_file(&self, locator: Locator) -> Result<File> {
        File::open(
            self.pool.clone(),
            self.branch.clone(),
            self.cryptor.clone(),
            locator,
        )
        .await
    }

    pub async fn open_directory(&self, locator: Locator) -> Result<Directory> {
        match Directory::open(
            self.pool.clone(),
            self.branch.clone(),
            self.cryptor.clone(),
            locator,
        )
        .await
        {
            Ok(dir) => Ok(dir),
            Err(Error::BlockIdNotFound) if locator == Locator::Root => {
                // Lazily Create the root directory
                Ok(Directory::create(
                    self.pool.clone(),
                    self.branch.clone(),
                    self.cryptor.clone(),
                    Locator::Root,
                ))
            }
            Err(error) => Err(error),
        }
    }
}
