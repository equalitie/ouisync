// From experience, this lint is almost never useful. Disabling it globally.
#![allow(clippy::large_enum_variant)]

#[macro_use]
mod macros;

pub mod crypto;
pub mod debug_printer;
pub mod device_id;

mod access_control;
mod blob;
mod blob_id;
mod block;
mod branch;
mod config;
mod db;
mod deadlock;
mod directory;
mod error;
mod ffi;
mod file;
mod format;
mod index;
mod iterator;
mod joint_directory;
mod joint_entry;
mod locator;
mod metadata;
mod network;
mod path;
mod repository;
mod scoped_task;
mod store;
mod sync;
#[cfg(test)]
mod test_utils;
mod version_vector;
mod versioned_file_name;

pub use self::{
    access_control::{AccessMode, AccessSecrets, MasterSecret, ShareToken},
    branch::Branch,
    config::ConfigStore,
    crypto::{cipher, sign, Password},
    db::Store,
    directory::{Directory, EntryRef, EntryType},
    error::{Error, Result},
    file::File,
    index::Index,
    joint_directory::{JointDirectory, JointEntryRef, MissingVersionStrategy},
    joint_entry::JointEntry,
    network::{Network, NetworkOptions},
    repository::Repository,
};
