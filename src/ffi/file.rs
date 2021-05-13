use super::{
    session,
    utils::{self, AssumeSend, Port, SharedHandle},
};
use crate::{error::Error, file::File, repository::Repository};
use std::{convert::TryInto, io::SeekFrom, os::raw::c_char, slice, sync::Arc};
use tokio::sync::Mutex;

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
            let (parent, name) = utils::decompose_path(&path).ok_or(Error::EntryExists)?;

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
            let (parent, name) = utils::decompose_path(&path).ok_or(Error::EntryExists)?;

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
