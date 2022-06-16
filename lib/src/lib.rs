// From experience, this lint is almost never useful. Disabling it globally.
#![allow(clippy::large_enum_variant)]

#[macro_use]
mod macros;

pub mod crypto;
pub mod db;
pub mod debug_printer;
pub mod device_id;
pub mod network;
pub mod path;
pub mod sync;

mod access_control;
mod blob;
mod blob_id;
mod block;
mod branch;
mod config;
mod deadlock;
mod directory;
mod error;
mod file;
mod format;
mod index;
mod iterator;
mod joint_directory;
mod joint_entry;
mod locator;
mod metadata;
mod progress;
mod repository;
mod scoped_task;
mod state_monitor;
mod store;
#[cfg(test)]
mod test_utils;
#[cfg_attr(test, macro_use)]
mod version_vector;
mod versioned_file_name;

pub use self::{
    access_control::{AccessMode, AccessSecrets, MasterSecret, ShareToken},
    blob::HEADER_SIZE as BLOB_HEADER_SIZE,
    block::BLOCK_SIZE,
    branch::Branch,
    config::ConfigStore,
    directory::{Directory, EntryRef, EntryType},
    error::{Error, Result},
    file::File,
    joint_directory::{JointDirectory, JointEntryRef, MissingVersionStrategy},
    joint_entry::JointEntry,
    repository::Repository,
    store::Store as RepositoryStore,
};
