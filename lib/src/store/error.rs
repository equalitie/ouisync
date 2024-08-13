use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database")]
    Db(#[from] sqlx::Error),
    #[error("malformed data")]
    MalformedData,
    #[error("branch not found")]
    BranchNotFound,
    #[error("attempt to create outdated root node")]
    OutdatedRootNode,
    #[error("attempt to create concurrent root node in the same branch")]
    ConcurrentRootNode,
    #[error("locator not found")]
    LocatorNotFound,
    #[error("block not found")]
    BlockNotFound,
}
