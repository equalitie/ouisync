use super::{
    dart::DartPort,
    session,
    utils::{self, RefHandle, SharedHandle, UniqueHandle},
};
use crate::{crypto::Cryptor, entry::EntryType, error::Error, file::File, repository::Repository};
use std::{
    convert::TryInto,
    ffi::{CString, OsStr},
    os::raw::c_char,
    path::Path,
    sync::Arc,
};
use thiserror::Error;
use tokio::sync::Mutex;

pub const DIR_ENTRY_FILE: u8 = 0;
pub const DIR_ENTRY_DIRECTORY: u8 = 1;

pub const OPEN_MODE_READ: u8 = 0b0000_0001;
pub const OPEN_MODE_CREATE: u8 = 0b000_0010;
pub const OPEN_MODE_APPEND: u8 = 0b0000_0100;
pub const OPEN_MODE_TRUNCATE: u8 = 0b0000_1000;

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
    // FIXME: erroring here would hang on the dart side because we never send to the port. This
    // applies to other functions as well.
    let path = try_ffi!(utils::ptr_to_path_buf(path), error);
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
    let path = try_ffi!(utils::ptr_to_path_buf(path), error);
    let repo = repo.get();

    session::spawn(port, error, async move {
        let (parent, name) = decompose_path(&path).ok_or(Error::EntryExists)?;

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

#[no_mangle]
pub unsafe extern "C" fn file_open(
    repo: SharedHandle<Repository>,
    path: *const c_char,
    mode: u8,
    port: DartPort,
    error: *mut *const c_char,
) {
    let path = try_ffi!(utils::ptr_to_path_buf(path), error);
    let repo = repo.get();

    try_ffi!(validate_open_mode(mode), error);

    session::spawn(port, error, async move {
        let (parent, name) = decompose_path(&path).ok_or(Error::EntryExists)?;

        let mut parent = repo.open_directory(parent).await?;

        let mut file = if mode & OPEN_MODE_CREATE != 0 {
            let file = parent.create_file(name.to_owned())?;
            parent.flush().await?;
            file
        } else {
            let entry = parent.lookup(name)?;
            entry.entry_type().check_is_file()?;
            entry.open().await?.try_into()?
        };

        if mode & OPEN_MODE_TRUNCATE != 0 {
            file.truncate().await?;
        }

        // TODO: set whether the file is read-only/write-only/read-write

        Ok(SharedHandle::new(Arc::new(Mutex::new(file))))
    })
}

#[no_mangle]
pub unsafe extern "C" fn file_close(
    handle: SharedHandle<Mutex<File>>,
    port: DartPort,
    error: *mut *const c_char,
) {
    let file = handle.release();
    session::spawn(port, error, async move { file.lock().await.flush().await })
}

#[no_mangle]
pub unsafe extern "C" fn file_flush(
    handle: SharedHandle<Mutex<File>>,
    port: DartPort,
    error: *mut *const c_char,
) {
    let file = handle.get();
    session::spawn(port, error, async move { file.lock().await.flush().await })
}

fn decompose_path(path: &Path) -> Option<(&Path, &OsStr)> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Some((parent, name)),
        _ => None,
    }
}

fn validate_open_mode(mode: u8) -> Result<(), InvalidOpenMode> {
    if mode & OPEN_MODE_READ == 0
        && mode & OPEN_MODE_CREATE == 0
        && mode & OPEN_MODE_TRUNCATE == 0
        && mode & OPEN_MODE_APPEND == 0
    {
        return Err(InvalidOpenMode::Empty);
    }

    if mode & OPEN_MODE_APPEND != 0 && mode & OPEN_MODE_TRUNCATE != 0 {
        // Conflicting modes
        return Err(InvalidOpenMode::Conflict);
    }

    Ok(())
}

#[derive(Debug, Error)]
#[error("invalid open mode")]
enum InvalidOpenMode {
    #[error("invalid open mode: APPEND and TRUNCATE cannot be used at the same time")]
    Conflict,
    #[error("invalid open mode: at least one flag needs to be specified")]
    Empty,
}
