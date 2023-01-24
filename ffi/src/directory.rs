use super::{
    registry::Handle,
    repository::{self, RepositoryHolder, ENTRY_TYPE_INVALID},
    session::SessionHandle,
    utils::{self, Port},
};
use ouisync_lib::{EntryType, Result};
use std::{convert::TryInto, ffi::CString, os::raw::c_char, ptr};

// Currently this is only a read-only snapshot of a directory.
pub struct Directory(Vec<DirEntry>);

impl Directory {
    fn get(&self, index: u64) -> Option<&DirEntry> {
        let index: usize = index.try_into().ok()?;
        self.0.get(index)
    }
}

struct DirEntry {
    name: CString,
    entry_type: EntryType,
}

#[no_mangle]
pub unsafe extern "C" fn directory_create(
    session: SessionHandle,
    repo: Handle<RepositoryHolder>,
    path: *const c_char,
    port: Port<Result<()>>,
) {
    session.get().with(port, |ctx| {
        let path = utils::ptr_to_path_buf(path)?;
        let repo = ctx.repositories().get(repo);

        ctx.spawn(async move {
            repo.repository.create_directory(path).await?;
            Ok(())
        })
    })
}

#[no_mangle]
pub unsafe extern "C" fn directory_open(
    session: SessionHandle,
    repo: Handle<RepositoryHolder>,
    path: *const c_char,
    port: Port<Result<Handle<Directory>>>,
) {
    session.get().with(port, |ctx| {
        let path = utils::ptr_to_path_buf(path)?;
        let repo = ctx.repositories().get(repo);
        let registry = ctx.directories().clone();

        ctx.spawn(async move {
            let dir = repo.repository.open_directory(path).await?;
            let entries = dir
                .entries()
                .map(|entry| DirEntry {
                    name: utils::str_to_c_string(&entry.unique_name()).unwrap_or_else(|_| {
                        CString::new(char::REPLACEMENT_CHARACTER.to_string()).unwrap()
                    }),
                    entry_type: entry.entry_type(),
                })
                .collect();
            let entries = Directory(entries);
            let handle = registry.insert(entries);

            Ok(handle)
        })
    })
}

/// Removes the directory at the given path from the repository. The directory must be empty.
#[no_mangle]
pub unsafe extern "C" fn directory_remove(
    session: SessionHandle,
    repo: Handle<RepositoryHolder>,
    path: *const c_char,
    port: Port<Result<()>>,
) {
    session.get().with(port, |ctx| {
        let repo = ctx.repositories().get(repo);
        let path = utils::ptr_to_path_buf(path)?;

        ctx.spawn(async move { repo.repository.remove_entry(path).await })
    })
}

/// Removes the directory at the given path including its content from the repository.
#[no_mangle]
pub unsafe extern "C" fn directory_remove_recursively(
    session: SessionHandle,
    repo: Handle<RepositoryHolder>,
    path: *const c_char,
    port: Port<Result<()>>,
) {
    session.get().with(port, |ctx| {
        let repo = ctx.repositories().get(repo);
        let path = utils::ptr_to_path_buf(path)?;

        ctx.spawn(async move { repo.repository.remove_entry_recursively(path).await })
    })
}

#[no_mangle]
pub unsafe extern "C" fn directory_close(session: SessionHandle, handle: Handle<Directory>) {
    session.get().directories.remove(handle);
}

#[no_mangle]
pub unsafe extern "C" fn directory_num_entries(
    session: SessionHandle,
    handle: Handle<Directory>,
) -> u64 {
    session.get().directories.get(handle).0.len() as u64
}

#[no_mangle]
pub unsafe extern "C" fn directory_entry_name(
    session: SessionHandle,
    handle: Handle<Directory>,
    index: u64,
) -> *const c_char {
    session
        .get()
        .directories
        .get(handle)
        .get(index)
        .map(|entry| entry.name.as_ptr())
        .unwrap_or(ptr::null())
}

#[no_mangle]
pub unsafe extern "C" fn directory_entry_type(
    session: SessionHandle,
    handle: Handle<Directory>,
    index: u64,
) -> u8 {
    session
        .get()
        .directories
        .get(handle)
        .get(index)
        .map(|entry| repository::entry_type_to_num(entry.entry_type))
        .unwrap_or(ENTRY_TYPE_INVALID)
}
