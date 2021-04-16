mod block;
mod block_store;
mod crypto;
mod error;
mod repository;

pub use self::{
    block::{BlockId, BlockName, BlockVersion, BLOCK_SIZE},
    block_store::BlockStore,
    error::Error,
    repository::Repository,
};

/// This function can be called from other languages via FFI
#[no_mangle]
pub extern "C" fn hello_ffi() {
    println!("Hello world")
}