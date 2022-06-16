use super::{
    session,
    utils::{self, AssumeSend, Port, SharedHandle},
};
use ouisync_lib::{sync::Mutex, Error, File, Repository, Result};
use std::{
    convert::TryInto,
    io::SeekFrom,
    os::raw::{c_char, c_int},
    slice,
    sync::Arc,
};

pub struct FfiFile {
    file: File,
    repo: Arc<Repository>,
}

#[no_mangle]
pub unsafe extern "C" fn file_open(
    repo: SharedHandle<Repository>,
    path: *const c_char,
    port: Port<Result<SharedHandle<Mutex<FfiFile>>>>,
) {
    session::with(port, |ctx| {
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
    port: Port<Result<SharedHandle<Mutex<FfiFile>>>>,
) {
    session::with(port, |ctx| {
        let path = utils::ptr_to_path_buf(path)?;
        let repo = repo.get();

        ctx.spawn(async move {
            let mut file = repo.create_file(&path).await?;
            let mut conn = repo.db().acquire().await?;
            file.flush(&mut conn).await?;

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
    port: Port<Result<()>>,
) {
    session::with(port, |ctx| {
        let repo = repo.get();
        let path = utils::ptr_to_path_buf(path)?;

        ctx.spawn(async move { repo.remove_entry(&path).await })
    })
}

#[no_mangle]
pub unsafe extern "C" fn file_close(handle: SharedHandle<Mutex<FfiFile>>, port: Port<Result<()>>) {
    session::with(port, |ctx| {
        let ffi_file = handle.release();

        ctx.spawn(async move {
            let mut g = ffi_file.lock().await;
            let mut conn = g.repo.db().acquire().await?;
            g.file.flush(&mut conn).await
        })
    })
}

#[no_mangle]
pub unsafe extern "C" fn file_flush(handle: SharedHandle<Mutex<FfiFile>>, port: Port<Result<()>>) {
    session::with(port, |ctx| {
        let ffi_file = handle.get();

        ctx.spawn(async move {
            let mut g = ffi_file.lock().await;
            let mut conn = g.repo.db().acquire().await?;
            g.file.flush(&mut conn).await
        })
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
    port: Port<Result<u64>>,
) {
    session::with(port, |ctx| {
        let ffi_file = handle.get();

        let buffer = AssumeSend::new(buffer);
        let len: usize = len.try_into().map_err(|_| Error::OffsetOutOfRange)?;

        ctx.spawn(async move {
            let mut g = ffi_file.lock().await;
            let mut conn = g.repo.db().acquire().await?;

            g.file.seek(&mut conn, SeekFrom::Start(offset)).await?;

            let buffer = slice::from_raw_parts_mut(buffer.into_inner(), len);
            let len = g.file.read(&mut conn, buffer).await? as u64;

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
    port: Port<Result<()>>,
) {
    session::with(port, |ctx| {
        let ffi_file = handle.get();

        let buffer = AssumeSend::new(buffer);
        let len: usize = len.try_into().map_err(|_| Error::OffsetOutOfRange)?;

        ctx.spawn(async move {
            let buffer = slice::from_raw_parts(buffer.into_inner(), len);

            let mut g = ffi_file.lock().await;

            let local_branch = g.repo.get_or_create_local_branch().await?;
            let mut conn = g.repo.db().acquire().await?;

            g.file.seek(&mut conn, SeekFrom::Start(offset)).await?;
            g.file.fork(&mut conn, &local_branch).await?;
            g.file.write(&mut conn, buffer).await?;

            Ok(())
        })
    })
}

/// Truncate the file to `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn file_truncate(
    handle: SharedHandle<Mutex<FfiFile>>,
    len: u64,
    port: Port<Result<()>>,
) {
    session::with(port, |ctx| {
        let ffi_file = handle.get();
        ctx.spawn(async move {
            let mut g = ffi_file.lock().await;

            let local_branch = g.repo.get_or_create_local_branch().await?;
            let mut conn = g.repo.db().acquire().await?;

            g.file.fork(&mut conn, &local_branch).await?;
            g.file.truncate(&mut conn, len).await
        })
    })
}

/// Retrieve the size of the file in bytes.
#[no_mangle]
pub unsafe extern "C" fn file_len(handle: SharedHandle<Mutex<FfiFile>>, port: Port<Result<u64>>) {
    session::with(port, |ctx| {
        let ffi_file = handle.get();
        ctx.spawn(async move {
            let g = ffi_file.lock().await;
            Ok(g.file.len().await)
        })
    })
}

/// Copy the file contents into the provided raw file descriptor.
/// This function takes ownership of the file descriptor and closes it when it finishes. If the
/// caller needs to access the descriptor afterwards (or while the function is running), he/she
/// needs to `dup` it before passing it into this function.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "C" fn file_copy_to_raw_fd(
    handle: SharedHandle<Mutex<FfiFile>>,
    fd: c_int,
    port: Port<Result<()>>,
) {
    use std::os::unix::io::FromRawFd;
    use tokio::fs;

    session::with(port, |ctx| {
        let src = handle.get();
        let mut dst = fs::File::from_raw_fd(fd);

        ctx.spawn(async move {
            let mut g = src.lock().await;
            let mut conn = g.repo.db().acquire().await?;
            g.file.copy_to_writer(&mut conn, &mut dst).await
        })
    })
}
