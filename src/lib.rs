pub mod crypto;
pub mod db;

mod async_object;
mod blob;
mod block;
mod client;
mod directory;
mod error;
mod file;
mod format;
mod index;
mod locator;
mod message;
mod message_broker;
mod network;
mod object_stream;
mod replica_discovery;
mod replica_id;
mod repository;
mod server;

pub use self::{
    async_object::AsyncObject,
    block::{BlockId, BlockName, BlockVersion, BLOCK_SIZE},
    client::Client,
    directory::Directory,
    error::Error,
    file::File,
    message::{Message, Request, Response},
    message_broker::MessageBroker,
    network::Network,
    object_stream::ObjectStream,
    replica_discovery::ReplicaDiscovery,
    replica_id::ReplicaId,
    repository::Repository,
    server::Server,
};

/// This function can be called from other languages via FFI
#[no_mangle]
pub extern "C" fn hello_ffi() {
    println!("Hello world")
}
