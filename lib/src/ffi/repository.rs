use super::{
    session,
    utils::{self, Port, SharedHandle, UniqueHandle},
};
use crate::{crypto::Cryptor, directory::EntryType, error::Error, path, repository::Repository};
use std::{os::raw::c_char, sync::Arc};
use tokio::task::JoinHandle;

pub const ENTRY_TYPE_INVALID: u8 = 0;
pub const ENTRY_TYPE_FILE: u8 = 1;
pub const ENTRY_TYPE_DIRECTORY: u8 = 2;

/// Opens a repository.
#[no_mangle]
pub unsafe extern "C" fn repository_open(
    store: *const c_char,
    port: Port<SharedHandle<Repository>>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let store = utils::ptr_to_path_buf(store)?;
        let network_handle = ctx.network().handle();

        ctx.spawn(async move {
            let repo = Repository::open(
                &store.into_std_path_buf().into(),
                *network_handle.this_replica_id(),
                Cryptor::Null,
                true,
            )
            .await?;

            network_handle.register(&repo).await;

            let repo = Arc::new(repo);
            Ok(SharedHandle::new(repo))
        })
    })
}

/// Closes a repository.
#[no_mangle]
pub unsafe extern "C" fn repository_close(handle: SharedHandle<Repository>) {
    handle.release();
}

/// Returns the type of repository entry (file, directory, ...).
/// If the entry doesn't exists, returns `ENTRY_TYPE_INVALID`, not an error.
#[no_mangle]
pub unsafe extern "C" fn repository_entry_type(
    handle: SharedHandle<Repository>,
    path: *const c_char,
    port: Port<u8>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let repo = handle.get();
        let path = utils::ptr_to_path_buf(path)?;

        ctx.spawn(async move {
            match repo.lookup_type(path).await {
                Ok(entry_type) => Ok(entry_type_to_num(entry_type)),
                Err(Error::EntryNotFound) => Ok(ENTRY_TYPE_INVALID),
                Err(error) => Err(error),
            }
        })
    })
}

/// Move/rename entry from src to dst.
#[no_mangle]
pub unsafe extern "C" fn repository_move_entry(
    handle: SharedHandle<Repository>,
    src: *const c_char,
    dst: *const c_char,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let repo = handle.get();
        let src = utils::ptr_to_path_buf(src)?;
        let dst = utils::ptr_to_path_buf(dst)?;

        ctx.spawn(async move {
            let (src_dir, src_name) = path::decompose(&src).ok_or(Error::EntryNotFound)?;
            let (dst_dir, dst_name) = path::decompose(&dst).ok_or(Error::EntryNotFound)?;

            repo.move_entry(src_dir, src_name, dst_dir, dst_name).await
        })
    })
}

/// Subscribe to change notifications from the repository.
#[no_mangle]
pub unsafe extern "C" fn repository_subscribe(
    handle: SharedHandle<Repository>,
    port: Port<()>,
) -> UniqueHandle<JoinHandle<()>> {
    let session = session::get();
    let sender = session.sender();
    let repo = handle.get();
    let mut rx = repo.subscribe();

    let handle = session.runtime().spawn(async move {
        loop {
            rx.recv().await;
            sender.send(port, ());
        }
    });

    UniqueHandle::new(Box::new(handle))
}

/// Cancel the repository change notifications subscription.
#[no_mangle]
pub unsafe extern "C" fn subscription_cancel(handle: UniqueHandle<JoinHandle<()>>) {
    handle.release().abort();
}

#[no_mangle]
pub unsafe extern "C" fn repository_is_dht_enabled(
    handle: UniqueHandle<Repository>,
    port: Port<bool>,
) {
    let session = session::get();
    let sender = session.sender();
    let network = session.network().handle();

    session.runtime().spawn(async move {
        let is_dht_enabled = network.is_dht_for_repository_enabled(handle.get()).await;
        sender.send(port, is_dht_enabled);
    });
}

#[no_mangle]
pub unsafe extern "C" fn repository_enable_dht(handle: UniqueHandle<Repository>, port: Port<()>) {
    let session = session::get();
    let sender = session.sender();
    let network = session.network().handle();

    session.runtime().spawn(async move {
        network.enable_dht_for_repository(handle.get()).await;
        sender.send(port, ());
    });
}

#[no_mangle]
pub unsafe extern "C" fn repository_disable_dht(handle: UniqueHandle<Repository>, port: Port<()>) {
    let session = session::get();
    let sender = session.sender();
    let network = session.network().handle();

    session.runtime().spawn(async move {
        network.disable_dht_for_repository(handle.get()).await;
        sender.send(port, ());
    });
}

pub(super) fn entry_type_to_num(entry_type: EntryType) -> u8 {
    match entry_type {
        EntryType::File => ENTRY_TYPE_FILE,
        EntryType::Directory => ENTRY_TYPE_DIRECTORY,
    }
}
