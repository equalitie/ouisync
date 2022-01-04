use super::{
    session,
    utils::{self, AssumeSend, Port, SharedHandle},
};
use crate::{error::Error, file::File, repository::Repository};
use std::{convert::TryInto, io::SeekFrom, os::raw::c_char, slice, sync::Arc};
use tokio::sync::Mutex;

pub struct FfiFile {
    file: File,
    repo: Arc<Repository>,
}

#[no_mangle]
pub unsafe extern "C" fn file_open(
    repo: SharedHandle<Repository>,
    path: *const c_char,
    port: Port<SharedHandle<Mutex<FfiFile>>>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let path = utils::ptr_to_path_buf(path)?;
        let repo = repo.get();

        ctx.spawn(async move {
            let file = repo.open_file(&path).await?;
            Ok(SharedHandle::new(Arc::new(Mutex::new(FfiFile {
                file,
                repo,
            }))))
        })
    })
}

#[no_mangle]
pub unsafe extern "C" fn file_create(
    repo: SharedHandle<Repository>,
    path: *const c_char,
    port: Port<SharedHandle<Mutex<FfiFile>>>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let path = utils::ptr_to_path_buf(path)?;
        let repo = repo.get();

        ctx.spawn(async move {
            let mut file = repo.create_file(&path).await?;
            file.flush().await?;

            Ok(SharedHandle::new(Arc::new(Mutex::new(FfiFile {
                file,
                repo,
            }))))
        })
    })
}

/// Remove (delete) the file at the given path from the repository.
#[no_mangle]
pub unsafe extern "C" fn file_remove(
    repo: SharedHandle<Repository>,
    path: *const c_char,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let repo = repo.get();
        let path = utils::ptr_to_path_buf(path)?;

        ctx.spawn(async move { repo.remove_entry(&path).await })
    })
}

#[no_mangle]
pub unsafe extern "C" fn file_close(
    handle: SharedHandle<Mutex<FfiFile>>,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let ffi_file = handle.release();

        ctx.spawn(async move { ffi_file.lock().await.file.flush().await })
    })
}

#[no_mangle]
pub unsafe extern "C" fn file_flush(
    handle: SharedHandle<Mutex<FfiFile>>,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let ffi_file = handle.get();

        ctx.spawn(async move { ffi_file.lock().await.file.flush().await })
    })
}

/// Read at most `len` bytes from the file into `buffer`. Yields the number of bytes actually read
/// (zero on EOF).
#[no_mangle]
pub unsafe extern "C" fn file_read(
    handle: SharedHandle<Mutex<FfiFile>>,
    offset: u64,
    buffer: *mut u8,
    len: u64,
    port: Port<u64>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let ffi_file = handle.get();

        let buffer = AssumeSend::new(buffer);
        let len: usize = len.try_into().map_err(|_| Error::OffsetOutOfRange)?;

        ctx.spawn(async move {
            let mut g = ffi_file.lock().await;
            g.file.seek(SeekFrom::Start(offset)).await?;

            let buffer = slice::from_raw_parts_mut(buffer.into_inner(), len);
            let len = g.file.read(buffer).await? as u64;

            Ok(len)
        })
    })
}

/// Write `len` bytes from `buffer` into the file.
#[no_mangle]
pub unsafe extern "C" fn file_write(
    handle: SharedHandle<Mutex<FfiFile>>,
    offset: u64,
    buffer: *const u8,
    len: u64,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let ffi_file = handle.get();

        let buffer = AssumeSend::new(buffer);
        let len: usize = len.try_into().map_err(|_| Error::OffsetOutOfRange)?;

        ctx.spawn(async move {
            let buffer = slice::from_raw_parts(buffer.into_inner(), len);

            let mut g = ffi_file.lock().await;

            let local_branch = g.repo.local_branch().await?;

            g.file.seek(SeekFrom::Start(offset)).await?;
            g.file.write(buffer, &local_branch).await?;

            Ok(())
        })
    })
}

/// Truncate the file to `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn file_truncate(
    handle: SharedHandle<Mutex<FfiFile>>,
    len: u64,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let ffi_file = handle.get();
        ctx.spawn(async move {
            let mut g = ffi_file.lock().await;
            let local_branch = g.repo.local_branch().await?;
            g.file.truncate(len, &local_branch).await
        })
    })
}

/// Retrieve the size of the file in bytes.
#[no_mangle]
pub unsafe extern "C" fn file_len(
    handle: SharedHandle<Mutex<FfiFile>>,
    port: Port<u64>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let ffi_file = handle.get();
        ctx.spawn(async move {
            let g = ffi_file.lock().await;
            Ok(g.file.len().await)
        })
    })
}
