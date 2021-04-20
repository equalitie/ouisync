pub mod crypto;
pub mod db;

mod blob;
mod block;
mod error;
mod format;
mod index;
mod replica_discovery;
mod repository;

pub use self::{
    block::{BlockId, BlockName, BlockStore, BlockVersion, BLOCK_SIZE},
    error::Error,
    replica_discovery::ReplicaDiscovery,
    repository::Repository,
};

/// This function can be called from other languages via FFI
#[no_mangle]
pub extern "C" fn hello_ffi() {
    println!("Hello world")
}
