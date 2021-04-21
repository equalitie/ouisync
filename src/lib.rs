pub mod crypto;
pub mod db;

mod async_object;
mod block;
mod error;
mod format;
mod index;
mod network;
mod object_stream;
mod replica_discovery;
mod replica_id;
mod repository;

pub use self::{
    async_object::AsyncObject,
    block::{BlockId, BlockName, BlockStore, BlockVersion, BLOCK_SIZE},
    error::Error,
    network::Network,
    object_stream::ObjectStream,
    replica_discovery::ReplicaDiscovery,
    replica_id::ReplicaId,
    repository::Repository,
};

/// This function can be called from other languages via FFI
#[no_mangle]
pub extern "C" fn hello_ffi() {
    println!("Hello world")
}
