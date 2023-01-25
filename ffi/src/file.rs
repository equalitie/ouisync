use super::{
    registry::Handle,
    repository::RepositoryHolder,
    session::SessionHandle,
    utils::{self, AssumeSend, Port},
};
use ouisync_lib::{deadlock::asynch::Mutex as AsyncMutex, Branch, Error, File, Result};
use std::{
    convert::TryInto,
    io::SeekFrom,
    os::raw::{c_char, c_int},
    slice,
};

pub struct FileHolder {
    file: AsyncMutex<File>,
    local_branch: Option<Branch>,
}

#[no_mangle]
pub unsafe extern "C" fn file_open(
    session: SessionHandle,
    repo: Handle<RepositoryHolder>,
    path: *const c_char,
    port: Port<Result<Handle<FileHolder>>>,
) {
    session.get().with(port, |ctx| {
        let path = utils::ptr_to_path_buf(path)?;
        let repo = ctx.state().repositories.get(repo);
        let local_branch = repo.repository.local_branch().ok();
        let state = ctx.state().clone();

        ctx.spawn(async move {
            let file = repo.repository.open_file(&path).await?;
            let holder = FileHolder {
                file: AsyncMutex::new(file),
                local_branch,
            };
            let handle = state.files.insert(holder);

            Ok(handle)
        })
    })
}

#[no_mangle]
pub unsafe extern "C" fn file_create(
    session: SessionHandle,
    repo: Handle<RepositoryHolder>,
    path: *const c_char,
    port: Port<Result<Handle<FileHolder>>>,
) {
    session.get().with(port, |ctx| {
        let path = utils::ptr_to_path_buf(path)?;
        let repo = ctx.state().repositories.get(repo);
        let local_branch = repo.repository.local_branch()?;
        let state = ctx.state().clone();

        ctx.spawn(async move {
            let mut file = repo.repository.create_file(&path).await?;
            file.flush().await?;

            let holder = FileHolder {
                file: AsyncMutex::new(file),
                local_branch: Some(local_branch),
            };
            let handle = state.files.insert(holder);

            Ok(handle)
        })
    })
}

/// Remove (delete) the file at the given path from the repository.
#[no_mangle]
pub unsafe extern "C" fn file_remove(
    session: SessionHandle,
    repo: Handle<RepositoryHolder>,
    path: *const c_char,
    port: Port<Result<()>>,
) {
    session.get().with(port, |ctx| {
        let repo = ctx.state().repositories.get(repo);
        let path = utils::ptr_to_path_buf(path)?;

        ctx.spawn(async move { repo.repository.remove_entry(&path).await })
    })
}

#[no_mangle]
pub unsafe extern "C" fn file_close(
    session: SessionHandle,
    handle: Handle<FileHolder>,
    port: Port<Result<()>>,
) {
    session.get().with(port, |ctx| {
        let holder = ctx.state().files.remove(handle);

        ctx.spawn(async move {
            if let Some(holder) = holder {
                holder.file.lock().await.flush().await
            } else {
                Ok(())
            }
        })
    })
}

#[no_mangle]
pub unsafe extern "C" fn file_flush(
    session: SessionHandle,
    handle: Handle<FileHolder>,
    port: Port<Result<()>>,
) {
    session.get().with(port, |ctx| {
        let holder = ctx.state().files.get(handle);
        ctx.spawn(async move { holder.file.lock().await.flush().await })
    })
}

/// Read at most `len` bytes from the file into `buffer`. Yields the number of bytes actually read
/// (zero on EOF).
#[no_mangle]
pub unsafe extern "C" fn file_read(
    session: SessionHandle,
    handle: Handle<FileHolder>,
    offset: u64,
    buffer: *mut u8,
    len: u64,
    port: Port<Result<u64>>,
) {
    session.get().with(port, |ctx| {
        let holder = ctx.state().files.get(handle);

        let buffer = AssumeSend::new(buffer);
        let len: usize = len.try_into().map_err(|_| Error::OffsetOutOfRange)?;

        ctx.spawn(async move {
            let mut file = holder.file.lock().await;

            file.seek(SeekFrom::Start(offset)).await?;

            let buffer = slice::from_raw_parts_mut(buffer.into_inner(), len);
            let len = file.read(buffer).await? as u64;

            Ok(len)
        })
    })
}

/// Write `len` bytes from `buffer` into the file.
#[no_mangle]
pub unsafe extern "C" fn file_write(
    session: SessionHandle,
    handle: Handle<FileHolder>,
    offset: u64,
    buffer: *const u8,
    len: u64,
    port: Port<Result<()>>,
) {
    session.get().with(port, |ctx| {
        let holder = ctx.state().files.get(handle);

        let buffer = AssumeSend::new(buffer);
        let len: usize = len.try_into().map_err(|_| Error::OffsetOutOfRange)?;

        ctx.spawn(async move {
            let buffer = slice::from_raw_parts(buffer.into_inner(), len);
            let mut file = holder.file.lock().await;

            let local_branch = holder
                .local_branch
                .as_ref()
                .ok_or(Error::PermissionDenied)?
                .clone();

            file.seek(SeekFrom::Start(offset)).await?;
            file.fork(local_branch).await?;
            file.write(buffer).await?;

            Ok(())
        })
    })
}

/// Truncate the file to `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn file_truncate(
    session: SessionHandle,
    handle: Handle<FileHolder>,
    len: u64,
    port: Port<Result<()>>,
) {
    session.get().with(port, |ctx| {
        let holder = ctx.state().files.get(handle);

        ctx.spawn(async move {
            let mut file = holder.file.lock().await;

            let local_branch = holder
                .local_branch
                .as_ref()
                .ok_or(Error::PermissionDenied)?
                .clone();

            file.fork(local_branch).await?;
            file.truncate(len).await?;

            Ok(())
        })
    })
}

/// Retrieve the size of the file in bytes.
#[no_mangle]
pub unsafe extern "C" fn file_len(
    session: SessionHandle,
    handle: Handle<FileHolder>,
    port: Port<Result<u64>>,
) {
    session.get().with(port, |ctx| {
        let holder = ctx.state().files.get(handle);

        ctx.spawn(async move { Ok(holder.file.lock().await.len()) })
    })
}

/// Copy the file contents into the provided raw file descriptor.
/// This function takes ownership of the file descriptor and closes it when it finishes. If the
/// caller needs to access the descriptor afterwards (or while the function is running), he/she
/// needs to `dup` it before passing it into this function.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "C" fn file_copy_to_raw_fd(
    session: SessionHandle,
    handle: Handle<FileHolder>,
    fd: c_int,
    port: Port<Result<()>>,
) {
    use std::os::unix::io::FromRawFd;
    use tokio::fs;

    session.get().with(port, |ctx| {
        let src = ctx.state().files.get(handle);
        let mut dst = fs::File::from_raw_fd(fd);

        ctx.spawn(async move {
            let mut src = src.file.lock().await;
            src.copy_to_writer(&mut dst).await
        })
    })
}
