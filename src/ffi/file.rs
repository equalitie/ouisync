use super::{
    session,
    utils::{self, AssumeSend, Port, SharedHandle},
};
use crate::{error::Error, file::File, repository::Repository};
use camino::Utf8PathBuf;
use std::{convert::TryInto, io::SeekFrom, os::raw::c_char, slice, sync::Arc};
use tokio::sync::Mutex;

pub struct HandleData {
    file: File,
    path: Utf8PathBuf,
    repo: Arc<Repository>,
}

#[no_mangle]
pub unsafe extern "C" fn file_open(
    repo: SharedHandle<Repository>,
    path: *const c_char,
    port: Port<SharedHandle<Mutex<HandleData>>>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let path = utils::ptr_to_utf8_path_buf(path)?;
        let repo = repo.get();

        ctx.spawn(async move {
            let file = repo.open_file(&path).await?;
            Ok(SharedHandle::new(Arc::new(Mutex::new(HandleData {
                file,
                path,
                repo,
            }))))
        })
    })
}

#[no_mangle]
pub unsafe extern "C" fn file_create(
    repo: SharedHandle<Repository>,
    path: *const c_char,
    port: Port<SharedHandle<Mutex<HandleData>>>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let path = utils::ptr_to_utf8_path_buf(path)?;
        let repo = repo.get();

        ctx.spawn(async move {
            let (mut file, mut parents) = repo.create_file(&path).await?;

            file.flush().await?;
            for dir in parents.iter_mut().rev() {
                dir.flush().await?;
            }

            Ok(SharedHandle::new(Arc::new(Mutex::new(HandleData {
                file,
                path,
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
        let path = utils::ptr_to_utf8_path_buf(path)?;

        ctx.spawn(async move { repo.remove_file(&path).await?.flush().await })
    })
}

#[no_mangle]
pub unsafe extern "C" fn file_close(
    handle: SharedHandle<Mutex<HandleData>>,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let handle_data = handle.release();
        ctx.spawn(async move { handle_data.lock().await.file.flush().await })
    })
}

#[no_mangle]
pub unsafe extern "C" fn file_flush(
    handle: SharedHandle<Mutex<HandleData>>,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let handle_data = handle.get();
        ctx.spawn(async move { handle_data.lock().await.file.flush().await })
    })
}

/// Read at most `len` bytes from the file into `buffer`. Yields the number of bytes actually read
/// (zero on EOF).
#[no_mangle]
pub unsafe extern "C" fn file_read(
    handle: SharedHandle<Mutex<HandleData>>,
    offset: u64,
    buffer: *mut u8,
    len: u64,
    port: Port<u64>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let handle_data = handle.get();

        let buffer = AssumeSend(buffer);
        let len: usize = len.try_into().map_err(|_| Error::OffsetOutOfRange)?;

        ctx.spawn(async move {
            let mut handle_data = handle_data.lock().await;
            handle_data.file.seek(SeekFrom::Start(offset)).await?;

            let buffer = slice::from_raw_parts_mut(buffer.0, len);
            let len = handle_data.file.read(buffer).await? as u64;

            Ok(len)
        })
    })
}

/// Write `len` bytes from `buffer` into the file.
#[no_mangle]
pub unsafe extern "C" fn file_write(
    handle: SharedHandle<Mutex<HandleData>>,
    offset: u64,
    buffer: *const u8,
    len: u64,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        use std::ops::DerefMut;

        let handle_data = handle.get();

        let buffer = AssumeSend(buffer);
        let len: usize = len.try_into().map_err(|_| Error::OffsetOutOfRange)?;

        ctx.spawn(async move {
            let mut handle_data = handle_data.lock().await;
            let handle_data = handle_data.deref_mut();

            let file = &mut handle_data.file;
            let repo = &handle_data.repo;
            let path = &handle_data.path;

            let buffer = slice::from_raw_parts(buffer.0, len);

            repo.write_to_file(path, file, offset, buffer).await?;

            Ok(())
        })
    })
}

/// Truncate the file to `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn file_truncate(
    handle: SharedHandle<Mutex<HandleData>>,
    len: u64,
    port: Port<()>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let handle_data = handle.get();
        ctx.spawn(async move { handle_data.lock().await.file.truncate(len).await })
    })
}

/// Retrieve the size of the file in bytes.
#[no_mangle]
pub unsafe extern "C" fn file_len(
    handle: SharedHandle<Mutex<HandleData>>,
    port: Port<u64>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let handle_data = handle.get();
        ctx.spawn(async move { Ok(handle_data.lock().await.file.len()) })
    })
}
