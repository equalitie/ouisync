pub mod crypto;
pub mod db;

mod block;
mod error;
mod format;
mod index;
mod repository;
mod replica_discovery;
mod object_stream;
mod replica_id;
mod network;

pub use self::{
    block::{BlockId, BlockName, BlockStore, BlockVersion, BLOCK_SIZE},
    error::Error,
    repository::Repository,
    replica_discovery::ReplicaDiscovery,
    object_stream::ObjectStream,
    replica_id::ReplicaId,
    network::Network,
};

/// This function can be called from other languages via FFI
#[no_mangle]
pub extern "C" fn hello_ffi() {
    println!("Hello world")
}
