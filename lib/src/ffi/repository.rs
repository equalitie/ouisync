use super::{
    session,
    utils::{self, Port, SharedHandle, UniqueHandle},
};
use crate::{
    access_control::{AccessSecrets, MasterSecret, ShareToken},
    crypto::Password,
    directory::EntryType,
    error::Error,
    network::Registration,
    path,
    repository::Repository,
};
use std::{os::raw::c_char, ptr, sync::Arc};
use tokio::task::JoinHandle;

pub const ENTRY_TYPE_INVALID: u8 = 0;
pub const ENTRY_TYPE_FILE: u8 = 1;
pub const ENTRY_TYPE_DIRECTORY: u8 = 2;

pub struct RepositoryHolder {
    repository: Repository,
    registration: Registration,
}

/// Opens a repository.
#[no_mangle]
pub unsafe extern "C" fn repository_open(
    store: *const c_char,
    master_password: *const c_char,
    port: Port<SharedHandle<RepositoryHolder>>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let store = utils::ptr_to_path_buf(store)?;
        let this_replica_id = *ctx.this_replica_id();
        let network_handle = ctx.network().handle();

        let master_password = if master_password.is_null() {
            Some(Password::new(utils::ptr_to_str(master_password)?))
        } else {
            None
        };

        ctx.spawn(async move {
            let repository = Repository::open(
                &store.into_std_path_buf().into(),
                this_replica_id,
                master_password.map(MasterSecret::Password),
                true,
            )
            .await?;

            let registration = network_handle.register(&repository).await;

            let holder = Arc::new(RepositoryHolder {
                repository,
                registration,
            });

            Ok(SharedHandle::new(holder))
        })
    })
}

/// Closes a repository.
#[no_mangle]
pub unsafe extern "C" fn repository_close(handle: SharedHandle<RepositoryHolder>) {
    // NOTE: The `Drop` impl of `Registration` `spawn`s which means it must be called from within a
    // runtime context.
    let _runtime_guard = session::get().runtime().enter();
    handle.release();
}

/// Returns the type of repository entry (file, directory, ...).
/// If the entry doesn't exists, returns `ENTRY_TYPE_INVALID`, not an error.
#[no_mangle]
pub unsafe extern "C" fn repository_entry_type(
    handle: SharedHandle<RepositoryHolder>,
    path: *const c_char,
    port: Port<u8>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let holder = handle.get();
        let path = utils::ptr_to_path_buf(path)?;

        ctx.spawn(async move {
            match holder.repository.lookup_type(path).await {
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
    handle: SharedHandle<RepositoryHolder>,
    src: *const c_char,
    dst: *const c_char,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let holder = handle.get();
        let src = utils::ptr_to_path_buf(src)?;
        let dst = utils::ptr_to_path_buf(dst)?;

        ctx.spawn(async move {
            let (src_dir, src_name) = path::decompose(&src).ok_or(Error::EntryNotFound)?;
            let (dst_dir, dst_name) = path::decompose(&dst).ok_or(Error::EntryNotFound)?;

            holder
                .repository
                .move_entry(src_dir, src_name, dst_dir, dst_name)
                .await
        })
    })
}

/// Subscribe to change notifications from the repository.
#[no_mangle]
pub unsafe extern "C" fn repository_subscribe(
    handle: SharedHandle<RepositoryHolder>,
    port: Port<()>,
) -> UniqueHandle<JoinHandle<()>> {
    let session = session::get();
    let sender = session.sender();
    let holder = handle.get();
    let mut rx = holder.repository.subscribe();

    let handle = session.runtime().spawn(async move {
        while rx.recv().await.is_ok() {
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
    handle: SharedHandle<RepositoryHolder>,
    port: Port<bool>,
) {
    let session = session::get();
    let sender = session.sender();
    let holder = handle.get();

    session.runtime().spawn(async move {
        let value = holder.registration.is_dht_enabled().await;
        sender.send(port, value);
    });
}

#[no_mangle]
pub unsafe extern "C" fn repository_enable_dht(
    handle: SharedHandle<RepositoryHolder>,
    port: Port<()>,
) {
    let session = session::get();
    let sender = session.sender();
    let holder = handle.get();

    session.runtime().spawn(async move {
        holder.registration.enable_dht().await;
        sender.send(port, ());
    });
}

#[no_mangle]
pub unsafe extern "C" fn repository_disable_dht(
    handle: SharedHandle<RepositoryHolder>,
    port: Port<()>,
) {
    let session = session::get();
    let sender = session.sender();
    let holder = handle.get();

    session.runtime().spawn(async move {
        holder.registration.disable_dht().await;
        sender.send(port, ());
    });
}

#[no_mangle]
pub unsafe extern "C" fn repository_create_share_token(
    handle: SharedHandle<RepositoryHolder>,
    name: *const c_char,
    port: Port<String>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let holder = handle.get();
        let name = utils::ptr_to_str(name)?.to_owned();

        ctx.spawn(async move {
            let id = holder
                .registration
                .get_or_create_id(&holder.repository)
                .await?;
            let share_token = ShareToken::from(AccessSecrets::Blind { id }).with_name(name);

            Ok(share_token.to_string())
        })
    })
}

#[no_mangle]
pub unsafe extern "C" fn repository_accept_share_token(
    handle: SharedHandle<RepositoryHolder>,
    token: *const c_char,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let holder = handle.get();
        let token = utils::ptr_to_str(token)?;
        let token: ShareToken = token.parse()?;

        ctx.spawn(async move {
            holder
                .registration
                .set_id(&holder.repository, *token.id())
                .await
        })
    })
}

/// IMPORTANT: the caller is responsible for deallocating the returned pointer unless it is `null`.
#[no_mangle]
pub unsafe extern "C" fn extract_suggested_name_from_share_token(
    token: *const c_char,
) -> *const c_char {
    let token = if let Ok(token) = utils::ptr_to_str(token) {
        token
    } else {
        return ptr::null();
    };

    let token: ShareToken = if let Ok(token) = token.parse() {
        token
    } else {
        return ptr::null();
    };

    if let Ok(s) = utils::str_to_c_string(token.suggested_name().as_ref()) {
        s.into_raw()
    } else {
        ptr::null()
    }
}

pub(super) fn entry_type_to_num(entry_type: EntryType) -> u8 {
    match entry_type {
        EntryType::File => ENTRY_TYPE_FILE,
        EntryType::Directory => ENTRY_TYPE_DIRECTORY,
    }
}
