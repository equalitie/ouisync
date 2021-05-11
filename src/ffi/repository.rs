use super::{
    dart::DartPort,
    session,
    utils::{self, RefHandle, SharedHandle, UniqueHandle},
};
use crate::{crypto::Cryptor, entry::EntryType, error::Error, repository::Repository};
use std::{
    convert::TryInto,
    ffi::{CStr, CString},
    os::raw::c_char,
    path::PathBuf,
    sync::Arc,
};

pub const DIR_ENTRY_FILE: u8 = 0;
pub const DIR_ENTRY_DIRECTORY: u8 = 1;

/// Opens a repository.
///
/// NOTE: eventually this function will allow to specify which repository to open, but currently
/// only one repository is supported.
#[no_mangle]
pub unsafe extern "C" fn repository_open(port: DartPort, error: *mut *const c_char) {
    // TODO: doesn't seems this needs to be async

    session::spawn(port, error, async move {
        let cryptor = Cryptor::Null; // TODO: support encryption
        let repo = Repository::new(session::pool()?.clone(), cryptor).await?;
        let repo = Arc::new(repo);

        Ok::<_, Error>(SharedHandle::new(repo))
    })
}

/// Closes a repository.
#[no_mangle]
pub unsafe extern "C" fn repository_close(handle: SharedHandle<Repository>) {
    handle.release();
}

#[no_mangle]
pub unsafe extern "C" fn repository_read_dir(
    repo: SharedHandle<Repository>,
    path: *const c_char,
    port: DartPort,
    error: *mut *const c_char,
) {
    let path = PathBuf::from(try_ffi!(
        utils::c_str_to_os_str(CStr::from_ptr(path)),
        error
    ));
    let repo = repo.get();

    session::spawn(port, error, async move {
        let dir = repo.open_directory(path).await?;
        let entries = dir
            .entries()
            .map(|info| DirEntry {
                name: utils::os_str_to_c_string(info.name()).unwrap_or_else(|_| {
                    CString::new(format!("{}", char::REPLACEMENT_CHARACTER)).unwrap()
                }),
                entry_type: info.entry_type(),
            })
            .collect();
        let entries = DirEntries(entries);
        let entries = Box::new(entries);

        Ok(UniqueHandle::new(entries))
    })
}

#[no_mangle]
pub unsafe extern "C" fn repository_create_dir(
    repo: SharedHandle<Repository>,
    path: *const c_char,
    port: DartPort,
    error: *mut *const c_char,
) {
    let path = PathBuf::from(try_ffi!(
        utils::c_str_to_os_str(CStr::from_ptr(path)),
        error
    ));
    let repo = repo.get();

    session::spawn(port, error, async move {
        let (parent, name) = match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => (parent, name),
            _ => return Err(Error::EntryExists),
        };

        let mut parent = repo.open_directory(parent).await?;
        let mut dir = parent.create_subdirectory(name.to_owned())?;

        dir.flush().await?;
        parent.flush().await?;

        Ok(())
    })
}

pub struct DirEntries(Vec<DirEntry>);

impl DirEntries {
    fn get(&self, index: u64) -> Option<&DirEntry> {
        let index: usize = index.try_into().ok()?;
        self.0.get(index)
    }
}

#[no_mangle]
pub unsafe extern "C" fn dir_entries_destroy(entries: UniqueHandle<DirEntries>) {
    let _ = entries.release();
}

#[no_mangle]
pub unsafe extern "C" fn dir_entries_count(entries: UniqueHandle<DirEntries>) -> u64 {
    entries.get().0.len() as u64
}

#[no_mangle]
pub unsafe extern "C" fn dir_entries_get(
    entries: UniqueHandle<DirEntries>,
    index: u64,
) -> RefHandle<DirEntry> {
    match entries.get().get(index) {
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
    match handle.get().entry_type {
        EntryType::File => DIR_ENTRY_FILE,
        EntryType::Directory => DIR_ENTRY_DIRECTORY,
    }
}
