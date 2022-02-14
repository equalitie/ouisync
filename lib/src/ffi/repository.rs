use super::{
    session,
    utils::{self, Bytes, Port, SharedHandle, UniqueHandle},
};
use crate::{
    access_control::{AccessMode, AccessSecrets, MasterSecret, ShareToken},
    crypto::Password,
    directory::EntryType,
    error::{Error, Result},
    network::Registration,
    path,
    repository::Repository,
};
use std::{os::raw::c_char, ptr, slice, sync::Arc};
use tokio::task::JoinHandle;

pub const ENTRY_TYPE_INVALID: u8 = 0;
pub const ENTRY_TYPE_FILE: u8 = 1;
pub const ENTRY_TYPE_DIRECTORY: u8 = 2;

pub const ACCESS_MODE_BLIND: u8 = 0;
pub const ACCESS_MODE_READ: u8 = 1;
pub const ACCESS_MODE_WRITE: u8 = 2;

// Merger is currently experimental.
const ENABLE_MERGER: bool = false;

pub struct RepositoryHolder {
    repository: Repository,
    registration: Registration,
}

/// Creates a new repository.
#[no_mangle]
pub unsafe extern "C" fn repository_create(
    store: *const c_char,
    master_password: *const c_char,
    share_token: *const c_char,
    port: Port<Result<SharedHandle<RepositoryHolder>>>,
) {
    session::with(port, |ctx| {
        let store = utils::ptr_to_path_buf(store)?;
        let device_id = *ctx.device_id();
        let network_handle = ctx.network().handle();

        let master_password = Password::new(utils::ptr_to_str(master_password)?);

        let access_secrets = if share_token.is_null() {
            AccessSecrets::random_write()
        } else {
            let share_token = utils::ptr_to_str(share_token)?;
            let share_token: ShareToken = share_token.parse()?;
            share_token.into_secrets()
        };

        ctx.spawn(async move {
            let repository = Repository::create(
                &store.into_std_path_buf().into(),
                device_id,
                MasterSecret::Password(master_password),
                access_secrets,
                ENABLE_MERGER,
            )
            .await?;

            let registration = network_handle.register(repository.index().clone()).await;

            let holder = Arc::new(RepositoryHolder {
                repository,
                registration,
            });

            Ok(SharedHandle::new(holder))
        })
    })
}

/// Opens an existing repository.
#[no_mangle]
pub unsafe extern "C" fn repository_open(
    store: *const c_char,
    master_password: *const c_char,
    port: Port<Result<SharedHandle<RepositoryHolder>>>,
) {
    session::with(port, |ctx| {
        let store = utils::ptr_to_path_buf(store)?;
        let device_id = *ctx.device_id();
        let network_handle = ctx.network().handle();

        let master_password = if master_password.is_null() {
            None
        } else {
            Some(Password::new(utils::ptr_to_str(master_password)?))
        };

        ctx.spawn(async move {
            let repository = Repository::open(
                &store.into_std_path_buf().into(),
                device_id,
                master_password.map(MasterSecret::Password),
                ENABLE_MERGER,
            )
            .await?;

            let registration = network_handle.register(repository.index().clone()).await;

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
    port: Port<Result<u8>>,
) {
    session::with(port, |ctx| {
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
    port: Port<Result<()>>,
) {
    session::with(port, |ctx| {
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
    access_mode: u8,
    name: *const c_char,
    port: Port<Result<String>>,
) {
    session::with(port, |ctx| {
        let holder = handle.get();
        let access_mode = access_mode_from_num(access_mode)?;
        let name = utils::ptr_to_str(name)?.to_owned();

        ctx.spawn(async move {
            let access_secrets = holder.repository.secrets().with_mode(access_mode);
            let share_token = ShareToken::from(access_secrets).with_name(name);

            Ok(share_token.to_string())
        })
    })
}

#[no_mangle]
pub unsafe extern "C" fn repository_access_mode(handle: SharedHandle<RepositoryHolder>) -> u8 {
    let holder = handle.get();
    access_mode_to_num(holder.repository.access_mode())
}

/// Returns the access mode of the given share token.
#[no_mangle]
pub unsafe extern "C" fn share_token_mode(token: *const c_char) -> u8 {
    #![allow(clippy::question_mark)] // false positive

    let token = if let Ok(token) = utils::ptr_to_str(token) {
        token
    } else {
        return ACCESS_MODE_BLIND;
    };

    let token: ShareToken = if let Ok(token) = token.parse() {
        token
    } else {
        return ACCESS_MODE_BLIND;
    };

    access_mode_to_num(token.access_mode())
}

/// IMPORTANT: the caller is responsible for deallocating the returned pointer unless it is `null`.
#[no_mangle]
pub unsafe extern "C" fn share_token_suggested_name(token: *const c_char) -> *const c_char {
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

/// IMPORTANT: the caller is responsible for deallocating the returned buffer unless it is `null`.
#[no_mangle]
pub unsafe extern "C" fn share_token_encode(token: *const c_char) -> Bytes {
    #![allow(clippy::question_mark)] // false positive

    let token = if let Ok(token) = utils::ptr_to_str(token) {
        token
    } else {
        return Bytes::NULL;
    };

    let token: ShareToken = if let Ok(token) = token.parse() {
        token
    } else {
        return Bytes::NULL;
    };

    let mut buffer = Vec::new();
    token.encode(&mut buffer);

    Bytes::from_vec(buffer)
}

/// IMPORTANT: the caller is responsible for deallocating the returned pointer unless it is `null`.
#[no_mangle]
pub unsafe extern "C" fn share_token_decode(bytes: *const u8, len: u64) -> *const c_char {
    let len = if let Ok(len) = len.try_into() {
        len
    } else {
        return ptr::null();
    };

    let slice = slice::from_raw_parts(bytes, len);

    let token = if let Ok(token) = ShareToken::decode(slice) {
        token
    } else {
        return ptr::null();
    };

    if let Ok(s) = utils::str_to_c_string(token.to_string().as_ref()) {
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

fn access_mode_from_num(num: u8) -> Result<AccessMode, Error> {
    // Note: we could've used `AccessMode::try_from` instead but then we would need a separate
    // check (ideally a compile-time one) that the `ACCESS_MODE_*` constants match the
    // corresponding `AccessMode` variants.

    match num {
        ACCESS_MODE_BLIND => Ok(AccessMode::Blind),
        ACCESS_MODE_READ => Ok(AccessMode::Read),
        ACCESS_MODE_WRITE => Ok(AccessMode::Write),
        _ => Err(Error::MalformedData),
    }
}

fn access_mode_to_num(mode: AccessMode) -> u8 {
    match mode {
        AccessMode::Blind => ACCESS_MODE_BLIND,
        AccessMode::Read => ACCESS_MODE_READ,
        AccessMode::Write => ACCESS_MODE_WRITE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_mode_constants() {
        for mode in [AccessMode::Blind, AccessMode::Read, AccessMode::Write] {
            assert_eq!(
                access_mode_from_num(access_mode_to_num(mode)).unwrap(),
                mode
            );
        }
    }
}
