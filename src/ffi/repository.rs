use super::{
    dart::DartPort,
    session,
    utils::{self, SharedHandle, UniqueHandle},
};
use crate::{crypto::Cryptor, db, entry::EntryType, error::Error, repository::Repository};
use std::{
    convert::TryInto,
    ffi::{CStr, CString},
    os::raw::c_char,
    path::PathBuf,
    ptr,
    sync::Arc,
};

pub type RepositoryHandle = SharedHandle<Repository>;
pub type DirEntriesHandle = UniqueHandle<DirEntries>;

/// Type of a directory entry.
pub const DIR_ENTRY_INVALID: u8 = 0;
pub const DIR_ENTRY_FILE: u8 = 1;
pub const DIR_ENTRY_DIRECTORY: u8 = 2;

pub struct DirEntries(Vec<DirEntry>);

impl DirEntries {
    fn get(&self, index: u64) -> Option<&DirEntry> {
        let index: usize = index.try_into().ok()?;
        self.0.get(index)
    }
}

struct DirEntry {
    name: CString,
    entry_type: EntryType,
}

/// Opens a repository.
///
/// NOTE: eventually this function will allow to specify which repository to open, but currently
/// only one repository is supported.
#[no_mangle]
pub unsafe extern "C" fn open_repository(
    store: *const c_char,
    port: DartPort,
    error: *mut *const c_char,
) {
    let store = CStr::from_ptr(store);
    let store = if store.to_bytes() == b":memory:" {
        db::Store::Memory
    } else {
        db::Store::File(PathBuf::from(try_ffi!(
            utils::c_str_to_os_str(store),
            error
        )))
    };

    log::trace!("open_repository {:?}", store);

    session::spawn(port, error, async move {
        let pool = db::init(store).await?;
        let cryptor = Cryptor::Null; // TODO: support encryption
        let repo = Repository::new(pool, cryptor).await?;
        let repo = Arc::new(repo);

        Ok::<_, Error>(RepositoryHandle::new(repo))
    })
}

/// Closes a repository.
#[no_mangle]
pub unsafe extern "C" fn close_repository(handle: RepositoryHandle) {
    handle.release();
}

#[no_mangle]
pub unsafe extern "C" fn read_dir(
    repo: RepositoryHandle,
    path: *const c_char,
    port: i64,
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

        Ok::<_, Error>(DirEntriesHandle::new(entries))
    })
}

#[no_mangle]
pub unsafe extern "C" fn destroy_dir_entries(entries: DirEntriesHandle) {
    let _ = entries.release();
}

#[no_mangle]
pub unsafe extern "C" fn dir_entries_count(entries: DirEntriesHandle) -> u64 {
    entries.get().0.len() as u64
}

#[no_mangle]
pub unsafe extern "C" fn dir_entries_name_at(
    entries: DirEntriesHandle,
    index: u64,
) -> *const c_char {
    match entries.get().get(index) {
        Some(entry) => entry.name.as_ptr(),
        None => ptr::null(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn dir_entries_type_at(entries: DirEntriesHandle, index: u64) -> u8 {
    match entries.get().get(index).map(|entry| entry.entry_type) {
        Some(EntryType::File) => DIR_ENTRY_FILE,
        Some(EntryType::Directory) => DIR_ENTRY_DIRECTORY,
        None => DIR_ENTRY_INVALID,
    }
}
