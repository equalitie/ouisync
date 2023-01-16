use super::{
    session,
    utils::{self, Bytes, Port, SharedHandle, UniqueHandle},
};
use ouisync_lib::{
    network::{self, Registration},
    path, Access, AccessMode, AccessSecrets, EntryType, Error, Event, LocalSecret, Payload,
    Repository, RepositoryDb, Result, ShareToken,
};
use std::{os::raw::c_char, ptr, slice, str::FromStr, sync::Arc};
use tokio::{sync::broadcast::error::RecvError, task::JoinHandle};
use tracing::Instrument;

pub const ENTRY_TYPE_INVALID: u8 = 0;
pub const ENTRY_TYPE_FILE: u8 = 1;
pub const ENTRY_TYPE_DIRECTORY: u8 = 2;

pub const ACCESS_MODE_BLIND: u8 = 0;
pub const ACCESS_MODE_READ: u8 = 1;
pub const ACCESS_MODE_WRITE: u8 = 2;

pub struct RepositoryHolder {
    pub(super) repository: Repository,
    registration: Registration,
}

/// Creates a new repository and set access to it based on the following table:
///
/// local_read_password  |  local_write_password  |  token access  |  result
/// ---------------------+------------------------+----------------+------------------------------
/// null or any          |  null or any           |  blind         |  blind replica
/// null                 |  null or any           |  read          |  read without password
/// read_pwd             |  null or any           |  read          |  read with read_pwd as password
/// null                 |  null                  |  write         |  read and write without password
/// any                  |  null                  |  write         |  read (only!) with password
/// null                 |  any                   |  write         |  read without password, require password for writing
/// any                  |  any                   |  write         |  read with password, write with (same or different) password
#[no_mangle]
pub unsafe extern "C" fn repository_create(
    store: *const c_char,
    local_read_password: *const c_char,
    local_write_password: *const c_char,
    share_token: *const c_char,
    port: Port<Result<SharedHandle<RepositoryHolder>>>,
) {
    session::with(port, |ctx| {
        let store = utils::ptr_to_path_buf(store)?;
        let device_id = *ctx.device_id();
        let network_handle = ctx.network().handle();

        let local_read_password = utils::ptr_to_pwd(local_read_password)?;
        let local_write_password = utils::ptr_to_pwd(local_write_password)?;

        let access_secrets = if share_token.is_null() {
            AccessSecrets::random_write()
        } else {
            let share_token = utils::ptr_to_str(share_token)?;
            let share_token: ShareToken = share_token.parse()?;
            share_token.into_secrets()
        };

        let span = ctx.repos_span().clone();

        ctx.spawn(
            async move {
                let db = RepositoryDb::create(store.into_std_path_buf()).await?;

                let local_read_key = if let Some(local_read_password) = local_read_password {
                    Some(db.password_to_key(local_read_password).await?)
                } else {
                    None
                };

                let local_write_key = if let Some(local_write_password) = local_write_password {
                    Some(db.password_to_key(local_write_password).await?)
                } else {
                    None
                };

                let access = Access::new(local_read_key, local_write_key, access_secrets);

                let repository = Repository::create(db, device_id, access).await?;

                let registration = network_handle.register(repository.store().clone());

                // TODO: consider leaving the decision to enable DHT, PEX to the app.
                registration.enable_dht();
                registration.enable_pex();

                let holder = Arc::new(RepositoryHolder {
                    repository,
                    registration,
                });

                Ok(SharedHandle::new(holder))
            }
            .instrument(span),
        )
    })
}

/// Opens an existing repository.
#[no_mangle]
pub unsafe extern "C" fn repository_open(
    store: *const c_char,
    local_password: *const c_char,
    port: Port<Result<SharedHandle<RepositoryHolder>>>,
) {
    session::with(port, |ctx| {
        let store = utils::ptr_to_path_buf(store)?;
        let device_id = *ctx.device_id();
        let network_handle = ctx.network().handle();

        let local_password = utils::ptr_to_pwd(local_password)?;

        let span = ctx.repos_span().clone();

        ctx.spawn(
            async move {
                let repository = Repository::open(
                    store.into_std_path_buf(),
                    device_id,
                    local_password.map(LocalSecret::Password),
                )
                .await?;

                let registration = network_handle.register(repository.store().clone());

                // TODO: consider leaving the decision to enable DHT, PEX to the app.
                registration.enable_dht();
                registration.enable_pex();

                let holder = Arc::new(RepositoryHolder {
                    repository,
                    registration,
                });

                Ok(SharedHandle::new(holder))
            }
            .instrument(span),
        )
    })
}

/// If `share_token` is null, the function will try with the currently active access secrets in the
/// repository. Note that passing `share_token` explicitly (as opposed to implicitly using the
/// currently active secrets) may be used to increase access permissions.
///
/// Attempting to change the secret without enough permissions will fail with PermissionDenied
/// error.
///
/// If `local_read_password` is null, the repository will become readable without a password.
/// To remove the read (and write) permission use the `repository_remove_read_access`
/// function.
#[no_mangle]
pub unsafe extern "C" fn repository_set_read_access(
    handle: SharedHandle<RepositoryHolder>,
    local_read_password: *const c_char,
    share_token: *const c_char,
    port: Port<Result<()>>,
) {
    session::with(port, |ctx| {
        let holder = handle.get();

        let access_secrets = if share_token.is_null() {
            // Repository shall attempt to use the one it's currently using.
            None
        } else {
            let share_token = utils::ptr_to_str(share_token)?;
            let share_token: ShareToken = share_token.parse()?;
            Some(share_token.into_secrets())
        };

        let local_read_secret = utils::ptr_to_pwd(local_read_password)?.map(LocalSecret::Password);

        ctx.spawn(async move {
            holder
                .repository
                .set_read_access(local_read_secret, access_secrets)
                .await
        })
    })
}

/// If `share_token` is null, the function will try with the currently active access secrets in the
/// repository. Note that passing `share_token` explicitly (as opposed to implicitly using the
/// currently active secrets) may be used to increase access permissions.
///
/// Attempting to change the secret without enough permissions will fail with PermissionDenied
/// error.
///
/// If `local_rw_password` is null, the repository will become read and writable without a
/// password.  To remove the read and write access use the
/// `repository_remove_read_and_write_access` function.
#[no_mangle]
pub unsafe extern "C" fn repository_set_read_and_write_access(
    handle: SharedHandle<RepositoryHolder>,
    local_rw_password: *const c_char,
    share_token: *const c_char,
    port: Port<Result<()>>,
) {
    session::with(port, |ctx| {
        let holder = handle.get();

        let access_secrets = if share_token.is_null() {
            // Repository shall attempt to use the one it's currently using.
            None
        } else {
            let share_token = utils::ptr_to_str(share_token)?;
            let share_token: ShareToken = share_token.parse()?;
            Some(share_token.into_secrets())
        };

        let local_rw_secret = utils::ptr_to_pwd(local_rw_password)?.map(LocalSecret::Password);

        ctx.spawn(async move {
            holder
                .repository
                .set_read_access(local_rw_secret.clone(), access_secrets.clone())
                .await?;

            holder
                .repository
                .set_write_access(local_rw_secret, access_secrets)
                .await?;

            Ok(())
        })
    })
}

/// Note that after removing read key the user may still read the repository if they previously had
/// write key set up.
#[no_mangle]
pub unsafe extern "C" fn repository_remove_read_key(
    handle: SharedHandle<RepositoryHolder>,
    port: Port<Result<()>>,
) {
    session::with(port, |ctx| {
        let holder = handle.get();
        ctx.spawn(async move { holder.repository.remove_read_key().await })
    })
}

/// Note that removing the write key will leave read key intact.
#[no_mangle]
pub unsafe extern "C" fn repository_remove_write_key(
    handle: SharedHandle<RepositoryHolder>,
    port: Port<Result<()>>,
) {
    session::with(port, |ctx| {
        let holder = handle.get();
        ctx.spawn(async move { holder.repository.remove_write_key().await })
    })
}

/// Closes a repository.
#[no_mangle]
pub unsafe extern "C" fn repository_close(
    handle: SharedHandle<RepositoryHolder>,
    port: Port<Result<()>>,
) {
    session::with(port, |ctx| {
        let holder = handle.release();
        ctx.spawn(async move { holder.repository.close().await })
    })
}

/// Returns true if the repository requires a local password to be opened for reading.
#[no_mangle]
pub unsafe extern "C" fn repository_requires_local_password_for_reading(
    handle: SharedHandle<RepositoryHolder>,
    port: Port<Result<bool>>,
) {
    session::with(port, |ctx| {
        let holder = handle.get();
        ctx.spawn(async move {
            holder
                .repository
                .requires_local_password_for_reading()
                .await
        })
    })
}

/// Returns true if the repository requires a local password to be opened for writing.
#[no_mangle]
pub unsafe extern "C" fn repository_requires_local_password_for_writing(
    handle: SharedHandle<RepositoryHolder>,
    port: Port<Result<bool>>,
) {
    session::with(port, |ctx| {
        let holder = handle.get();
        ctx.spawn(async move {
            holder
                .repository
                .requires_local_password_for_writing()
                .await
        })
    })
}

/// Return the RepositoryId of the repository in the low hex format.
/// User is responsible for deallocating the returned string.
#[deprecated = "use repository_info_hash instead"]
#[no_mangle]
pub unsafe extern "C" fn repository_low_hex_id(
    handle: SharedHandle<RepositoryHolder>,
) -> *const c_char {
    let holder = handle.get();
    utils::str_to_ptr(&hex::encode(holder.repository.secrets().id().as_ref()))
}

/// Return the info-hash of the repository formatted as hex string. This can be used as a globally
/// unique, non-secret identifier of the repository.
/// User is responsible for deallocating the returned string.
#[no_mangle]
pub unsafe extern "C" fn repository_info_hash(
    handle: SharedHandle<RepositoryHolder>,
) -> *const c_char {
    let holder = handle.get();
    utils::str_to_ptr(&hex::encode(
        network::repository_info_hash(holder.repository.secrets().id()).as_ref(),
    ))
}

/// Returns an ID that is randomly generated once per repository. Can be used to store local user
/// data per repository (e.g. passwords behind biometric storage).
#[no_mangle]
pub unsafe extern "C" fn repository_database_id(
    handle: SharedHandle<RepositoryHolder>,
    port: Port<Result<Vec<u8>>>,
) {
    session::with(port, |ctx| {
        let holder = handle.get();
        ctx.spawn(async move { Ok(holder.repository.database_id().await?.as_ref().to_vec()) })
    })
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
        loop {
            match rx.recv().await {
                // Only `BlockReceived` events cause user-observable changes
                Ok(Event {
                    payload: Payload::BlockReceived { .. },
                    ..
                }) => (),
                Ok(Event {
                    payload: Payload::BranchChanged(_) | Payload::FileClosed,
                    ..
                }) => continue,
                Err(RecvError::Lagged(_)) => (),
                Err(RecvError::Closed) => break,
            }

            sender.send(port, ());
        }
    });

    UniqueHandle::new(Box::new(handle))
}

#[no_mangle]
pub unsafe extern "C" fn repository_is_dht_enabled(handle: SharedHandle<RepositoryHolder>) -> bool {
    handle.get().registration.is_dht_enabled()
}

#[no_mangle]
pub unsafe extern "C" fn repository_enable_dht(handle: SharedHandle<RepositoryHolder>) {
    let session = session::get();
    let holder = handle.get();

    // HACK: the `enable_dht` call isn't async so spawning it should not be necessary. However,
    // calling it directly (even with entered runtime context) sometimes causes crash in the app
    // (SIGSEGV / stack corruption) for some reason. The spawn seems to fix it.
    let task = session
        .runtime()
        .spawn(async move { holder.registration.enable_dht() });

    // HACK: wait until the task completes so that this function is actually sync.
    session.runtime().block_on(task).ok();
}

#[no_mangle]
pub unsafe extern "C" fn repository_disable_dht(handle: SharedHandle<RepositoryHolder>) {
    handle.get().registration.disable_dht()
}

#[no_mangle]
pub unsafe extern "C" fn repository_is_pex_enabled(handle: SharedHandle<RepositoryHolder>) -> bool {
    handle.get().registration.is_pex_enabled()
}

#[no_mangle]
pub unsafe extern "C" fn repository_enable_pex(handle: SharedHandle<RepositoryHolder>) {
    handle.get().registration.enable_pex()
}

#[no_mangle]
pub unsafe extern "C" fn repository_disable_pex(handle: SharedHandle<RepositoryHolder>) {
    handle.get().registration.disable_pex()
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

/// Returns the syncing progress as a float in the 0.0 - 1.0 range.
#[no_mangle]
pub unsafe extern "C" fn repository_sync_progress(
    handle: SharedHandle<RepositoryHolder>,
    port: Port<Result<Vec<u8>>>,
) {
    session::with(port, |ctx| {
        let holder = handle.get();
        ctx.spawn(async move {
            let progress = holder.repository.sync_progress().await?;
            // unwrap is OK because serialization into a vector has no reason to fail
            Ok(rmp_serde::to_vec(&progress).unwrap())
        })
    })
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

/// Return the RepositoryId of the repository corresponding to the share token in the low hex format.
/// User is responsible for deallocating the returned string.
#[deprecated = "use share_token_info_hash instead"]
#[no_mangle]
pub unsafe extern "C" fn share_token_repository_low_hex_id(token: *const c_char) -> *const c_char {
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

    utils::str_to_ptr(&hex::encode(token.id().as_ref()))
}

/// Returns the info-hash of the repository corresponding to the share token formatted as hex
/// string.
/// User is responsible for deallocating the returned string.
#[no_mangle]
pub unsafe extern "C" fn share_token_info_hash(token: *const c_char) -> *const c_char {
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

    utils::str_to_ptr(&hex::encode(
        network::repository_info_hash(token.id()).as_ref(),
    ))
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

    utils::str_to_ptr(token.suggested_name().as_ref())
}

/// Take the input string, decide whether it's a valid OuiSync token and normalize it (remove white
/// space, unnecessary slashes,...).
/// IMPORTANT: the caller is responsible for deallocating the returned buffer unless it is `null`.
#[no_mangle]
pub unsafe extern "C" fn share_token_normalize(token: *const c_char) -> *const c_char {
    #![allow(clippy::question_mark)] // false positive

    let token = if let Ok(token) = utils::ptr_to_str(token) {
        token
    } else {
        return ptr::null();
    };

    let token: ShareToken = if let Ok(token) = ShareToken::from_str(token) {
        token
    } else {
        return ptr::null();
    };

    utils::str_to_ptr(&format!("{}", token))
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

    utils::str_to_ptr(token.to_string().as_ref())
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
