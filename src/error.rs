use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("block store error")]
    BlockStore(#[from] crate::block_store::Error),
}
