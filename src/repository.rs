use crate::{
    crypto::Cryptor,
    db,
    directory::Directory,
    error::{Error, Result},
    locator::Locator,
};

pub struct Repository {
    pool: db::Pool,
    cryptor: Cryptor,
}

impl Repository {
    pub fn new(pool: db::Pool, cryptor: Cryptor) -> Self {
        Self { pool, cryptor }
    }

    /// Open a directory.
    pub async fn open_directory(&self, locator: Locator) -> Result<Directory> {
        match Directory::open(self.pool.clone(), self.cryptor.clone(), locator).await {
            Ok(dir) => Ok(dir),
            Err(Error::BlockIdNotFound) if locator == Locator::Root => {
                // Lazily Create the root directory
                Ok(Directory::create(
                    self.pool.clone(),
                    self.cryptor.clone(),
                    Locator::Root,
                ))
            }
            Err(error) => Err(error),
        }
    }
}
