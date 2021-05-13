use super::{
    session,
    utils::{self, AssumeSend, Port, RefHandle, SharedHandle, UniqueHandle},
};
use crate::{crypto::Cryptor, entry::EntryType, error::Error, file::File, repository::Repository};
use std::{
    convert::TryInto,
    ffi::{CString, OsStr},
    io::SeekFrom,
    os::raw::c_char,
    path::Path,
    slice,
    sync::Arc,
};
use tokio::sync::Mutex;

pub const DIR_ENTRY_FILE: u8 = 0;
pub const DIR_ENTRY_DIRECTORY: u8 = 1;

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
        let pool = ctx.pool().clone();
        let cryptor = Cryptor::Null; // TODO: support encryption

        ctx.spawn(async move {
            let repo = Repository::new(pool, cryptor).await?;
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
        let path = utils::ptr_to_path_buf(path)?;
        let repo = repo.get();

        ctx.spawn(async move {
            let (parent, name) = decompose_path(&path).ok_or(Error::EntryExists)?;

            let mut parent = repo.open_directory(parent).await?;
            let mut dir = parent.create_subdirectory(name.to_owned())?;

            dir.flush().await?;
            parent.flush().await?;

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
        let path = utils::ptr_to_path_buf(path)?;
        let repo = repo.get();

        ctx.spawn(async move {
            let dir = repo.open_directory(path).await?;
            let entries = dir
                .entries()
                .map(|info| DirEntry {
                    name: utils::os_str_to_c_string(info.name()).unwrap_or_else(|_| {
                        CString::new(char::REPLACEMENT_CHARACTER.to_string()).unwrap()
                    }),
                    entry_type: info.entry_type(),
                })
                .collect();
            let entries = Directory(entries);
            let entries = Box::new(entries);

            Ok(UniqueHandle::new(entries))
        })
    })
}

#[no_mangle]
pub unsafe extern "C" fn directory_close(handle: UniqueHandle<Directory>) {
    let _ = handle.release();
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
    match handle.get().entry_type {
        EntryType::File => DIR_ENTRY_FILE,
        EntryType::Directory => DIR_ENTRY_DIRECTORY,
    }
}

#[no_mangle]
pub unsafe extern "C" fn file_open(
    repo: SharedHandle<Repository>,
    path: *const c_char,
    port: Port<SharedHandle<Mutex<File>>>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let path = utils::ptr_to_path_buf(path)?;
        let repo = repo.get();

        ctx.spawn(async move {
            let (parent, name) = decompose_path(&path).ok_or(Error::EntryExists)?;

            let file: File = repo
                .open_directory(parent)
                .await?
                .lookup(name)?
                .open()
                .await?
                .try_into()?;

            Ok(SharedHandle::new(Arc::new(Mutex::new(file))))
        })
    })
}

#[no_mangle]
pub unsafe extern "C" fn file_create(
    repo: SharedHandle<Repository>,
    path: *const c_char,
    port: Port<SharedHandle<Mutex<File>>>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let path = utils::ptr_to_path_buf(path)?;
        let repo = repo.get();

        ctx.spawn(async move {
            let (parent, name) = decompose_path(&path).ok_or(Error::EntryExists)?;

            let mut parent = repo.open_directory(parent).await?;
            let mut file = parent.create_file(name.to_owned())?;

            file.flush().await?;
            parent.flush().await?;

            Ok(SharedHandle::new(Arc::new(Mutex::new(file))))
        })
    })
}

#[no_mangle]
pub unsafe extern "C" fn file_close(
    handle: SharedHandle<Mutex<File>>,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    let file = handle.release();
    session::with(port, error, |ctx| {
        ctx.spawn(async move { file.lock().await.flush().await })
    })
}

#[no_mangle]
pub unsafe extern "C" fn file_flush(
    handle: SharedHandle<Mutex<File>>,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    let file = handle.get();
    session::with(port, error, |ctx| {
        ctx.spawn(async move { file.lock().await.flush().await })
    })
}

/// Read at most `len` bytes from the file into `buffer`. Yields the number of bytes actually read
/// (zero on EOF).
#[no_mangle]
pub unsafe extern "C" fn file_read(
    handle: SharedHandle<Mutex<File>>,
    offset: u64,
    buffer: *mut u8,
    len: u64,
    port: Port<u64>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let file = handle.get();

        let buffer = AssumeSend(buffer);
        let len: usize = len.try_into().map_err(|_| Error::OffsetOutOfRange)?;

        ctx.spawn(async move {
            let mut file = file.lock().await;
            file.seek(SeekFrom::Start(offset)).await?;

            let buffer = slice::from_raw_parts_mut(buffer.0, len);
            let len = file.read(buffer).await? as u64;

            Ok(len)
        })
    })
}

/// Write `len` bytes from `buffer` into the file.
#[no_mangle]
pub unsafe extern "C" fn file_write(
    handle: SharedHandle<Mutex<File>>,
    offset: u64,
    buffer: *const u8,
    len: u64,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let file = handle.get();

        let buffer = AssumeSend(buffer);
        let len: usize = len.try_into().map_err(|_| Error::OffsetOutOfRange)?;

        ctx.spawn(async move {
            let mut file = file.lock().await;
            file.seek(SeekFrom::Start(offset)).await?;

            let buffer = slice::from_raw_parts(buffer.0, len);
            file.write(buffer).await?;

            Ok(())
        })
    })
}

/// Truncate the file to `len` bytes.
// TODO: `len` is currently ignored and is always set to zero.
#[no_mangle]
pub unsafe extern "C" fn file_truncate(
    handle: SharedHandle<Mutex<File>>,
    _len: u64,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let file = handle.get();
        ctx.spawn(async move { file.lock().await.truncate().await })
    })
}

fn decompose_path(path: &Path) -> Option<(&Path, &OsStr)> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Some((parent, name)),
        _ => None,
    }
}
