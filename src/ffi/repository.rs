use super::{
    session,
    utils::{self, Port, SharedHandle},
};
use crate::{entry::EntryType, error::Error, repository::Repository};
use std::{os::raw::c_char, sync::Arc};

pub const ENTRY_TYPE_INVALID: u8 = 0;
pub const ENTRY_TYPE_FILE: u8 = 1;
pub const ENTRY_TYPE_DIRECTORY: u8 = 2;

/// Opens a repository.
///
/// NOTE: eventually this function will allow to specify which repository to open, but currently
/// only one repository is supported.
#[no_mangle]
pub unsafe extern "C" fn repository_open(
    port: Port<SharedHandle<Repository>>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let repo = ctx.session().open_repository();

        ctx.spawn(async move {
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
        let path = utils::ptr_to_utf8_path_buf(path)?;

        ctx.spawn(async move {
            match repo.lookup(path).await {
                Ok((_, entry_type)) => Ok(entry_type_to_num(entry_type)),
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
        let src = utils::ptr_to_utf8_path_buf(src)?;
        let dst = utils::ptr_to_utf8_path_buf(dst)?;

        ctx.spawn(async move {
            let (mut src_parent, mut dst_parent) = repo.move_entry(src, dst).await?;
            src_parent.flush().await?;
            dst_parent.get(&mut src_parent).flush().await?;
            Ok(())
        })
    })
}

pub(super) fn entry_type_to_num(entry_type: EntryType) -> u8 {
    match entry_type {
        EntryType::File => ENTRY_TYPE_FILE,
        EntryType::Directory => ENTRY_TYPE_DIRECTORY,
    }
}
