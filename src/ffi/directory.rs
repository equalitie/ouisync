use super::{
    repository, session,
    utils::{self, Port, RefHandle, SharedHandle, UniqueHandle},
};
use crate::{entry_type::EntryType, repository::Repository};
use std::{convert::TryInto, ffi::CString, os::raw::c_char};

// Currently this is only a read-only snapshot of a directory.
pub struct Directory(Vec<DirEntry>);

impl Directory {
    fn get(&self, index: u64) -> Option<&DirEntry> {
        let index: usize = index.try_into().ok()?;
        self.0.get(index)
    }
}

#[no_mangle]
pub unsafe extern "C" fn directory_create(
    repo: SharedHandle<Repository>,
    path: *const c_char,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let path = utils::ptr_to_utf8_path_buf(path)?;
        let repo = repo.get();

        ctx.spawn(async move {
            let mut dir = repo.create_directory(path).await?;
            dir.flush(None).await?;

            Ok(())
        })
    })
}

#[no_mangle]
pub unsafe extern "C" fn directory_open(
    repo: SharedHandle<Repository>,
    path: *const c_char,
    port: Port<UniqueHandle<Directory>>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let path = utils::ptr_to_utf8_path_buf(path)?;
        let repo = repo.get();

        ctx.spawn(async move {
            let dir = repo.open_directory(path).await?;
            let entries = dir
                .read()
                .await
                .entries()
                .map(|entry| DirEntry {
                    // TODO: use the dismbiguated name here
                    name: utils::str_to_c_string(entry.name()).unwrap_or_else(|_| {
                        CString::new(char::REPLACEMENT_CHARACTER.to_string()).unwrap()
                    }),
                    entry_type: entry.entry_type(),
                })
                .collect();
            let entries = Directory(entries);
            let entries = Box::new(entries);

            Ok(UniqueHandle::new(entries))
        })
    })
}

/// Remove (delete) the directory at the given path from the repository.
#[no_mangle]
pub unsafe extern "C" fn directory_remove(
    repo: SharedHandle<Repository>,
    path: *const c_char,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let repo = repo.get();
        let path = utils::ptr_to_utf8_path_buf(path)?;

        ctx.spawn(async move { repo.remove_directory(&path).await?.flush().await })
    })
}

#[no_mangle]
pub unsafe extern "C" fn directory_close(handle: UniqueHandle<Directory>) {
    handle.release();
}

#[no_mangle]
pub unsafe extern "C" fn directory_num_entries(handle: UniqueHandle<Directory>) -> u64 {
    handle.get().0.len() as u64
}

#[no_mangle]
pub unsafe extern "C" fn directory_get_entry(
    handle: UniqueHandle<Directory>,
    index: u64,
) -> RefHandle<DirEntry> {
    match handle.get().get(index) {
        Some(entry) => RefHandle::new(entry),
        None => RefHandle::NULL,
    }
}

pub struct DirEntry {
    name: CString,
    entry_type: EntryType,
}

#[no_mangle]
pub unsafe extern "C" fn dir_entry_name(handle: RefHandle<DirEntry>) -> *const c_char {
    handle.get().name.as_ptr()
}

#[no_mangle]
pub unsafe extern "C" fn dir_entry_type(handle: RefHandle<DirEntry>) -> u8 {
    repository::entry_type_to_num(handle.get().entry_type)
}
