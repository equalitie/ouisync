// From experience, this lint is almost never useful. Disabling it globally.
#![allow(clippy::large_enum_variant)]

#[macro_use]
mod macros;

pub mod crypto;
pub mod db;
pub mod path;
pub mod protocol;

mod access_control;
mod blob;
mod block_tracker;
mod branch;
mod collections;
mod conflict;
mod debug;
mod device_id;
mod directory;
mod error;
mod event;
mod file;
mod future;
mod iterator;
mod joint_directory;
mod joint_entry;
mod network;
mod progress;
mod repository;
mod store;
mod sync;
#[cfg(test)]
mod test_utils;
mod time;
#[cfg_attr(test, macro_use)]
mod version_vector;
mod versioned;

pub use self::{
    access_control::{
        Access, AccessChange, AccessMode, AccessSecrets, DecodeError, KeyAndSalt, LocalSecret,
        SetLocalSecret, ShareToken, WriteSecrets,
    },
    blob::HEADER_SIZE as BLOB_HEADER_SIZE,
    branch::Branch,
    db::SCHEMA_VERSION,
    debug::DebugPrinter,
    device_id::DeviceId,
    directory::{Directory, EntryRef, EntryType, DIRECTORY_VERSION},
    error::{Error, Result},
    event::{Event, Payload},
    file::File,
    joint_directory::{JointDirectory, JointEntryRef},
    joint_entry::JointEntry,
    network::{
        repository_info_hash, DhtContactsStoreTrait, NatBehavior, Network, NetworkEvent,
        NetworkEventReceiver, NetworkEventStream, PeerAddr, PeerInfo, PeerInfoCollector,
        PeerSource, PeerState, PublicRuntimeId, Registration, SecretRuntimeId, Stats, DHT_ROUTERS,
    },
    progress::Progress,
    protocol::{RepositoryId, StorageSize, BLOCK_SIZE},
    repository::{
        database_files, Credentials, Metadata, Repository, RepositoryHandle, RepositoryParams,
    },
    store::{Error as StoreError, DATA_VERSION},
    version_vector::VersionVector,
};
