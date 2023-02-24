use crate::{
    error::{Error, Result},
    registry::Handle,
    repository::RepositoryHolder,
    session::SessionHandle,
    state::ServerState,
    utils::Port,
};
use camino::Utf8PathBuf;
use ouisync_lib::{deadlock::asynch::Mutex as AsyncMutex, Branch, File};
use std::{convert::TryInto, io::SeekFrom, os::raw::c_int};

pub struct FileHolder {
    file: AsyncMutex<File>,
    local_branch: Option<Branch>,
}

pub(crate) async fn open(
    state: &ServerState,
    repo: Handle<RepositoryHolder>,
    path: Utf8PathBuf,
) -> Result<Handle<FileHolder>> {
    let repo = state.repositories.get(repo);
    let local_branch = repo.repository.local_branch().ok();

    let file = repo.repository.open_file(&path).await?;
    let holder = FileHolder {
        file: AsyncMutex::new(file),
        local_branch,
    };
    let handle = state.files.insert(holder);

    Ok(handle)
}

pub(crate) async fn create(
    state: &ServerState,
    repo: Handle<RepositoryHolder>,
    path: Utf8PathBuf,
) -> Result<Handle<FileHolder>> {
    let repo = state.repositories.get(repo);
    let local_branch = repo.repository.local_branch()?;

    let file = repo.repository.create_file(&path).await?;
    let holder = FileHolder {
        file: AsyncMutex::new(file),
        local_branch: Some(local_branch),
    };
    let handle = state.files.insert(holder);

    Ok(handle)
}

/// Remove (delete) the file at the given path from the repository.
pub(crate) async fn remove(
    state: &ServerState,
    repo: Handle<RepositoryHolder>,
    path: Utf8PathBuf,
) -> Result<()> {
    state
        .repositories
        .get(repo)
        .repository
        .remove_entry(&path)
        .await?;
    Ok(())
}

pub(crate) async fn close(state: &ServerState, handle: Handle<FileHolder>) -> Result<()> {
    if let Some(holder) = state.files.remove(handle) {
        holder.file.lock().await.flush().await?
    }

    Ok(())
}

pub(crate) async fn flush(state: &ServerState, handle: Handle<FileHolder>) -> Result<()> {
    state.files.get(handle).file.lock().await.flush().await?;
    Ok(())
}

/// Read at most `len` bytes from the file and returns them. The returned buffer can be shorter
/// than `len` and empty in case of EOF.
pub(crate) async fn read(
    state: &ServerState,
    handle: Handle<FileHolder>,
    offset: u64,
    len: u64,
) -> Result<Vec<u8>> {
    let len: usize = len.try_into().map_err(|_| Error::InvalidArgument)?;
    let mut buffer = vec![0; len];

    let holder = state.files.get(handle);
    let mut file = holder.file.lock().await;

    file.seek(SeekFrom::Start(offset)).await?;

    let len = file.read(&mut buffer).await?;
    buffer.truncate(len);

    Ok(buffer)
}

/// Write `len` bytes from `buffer` into the file.
pub(crate) async fn write(
    state: &ServerState,
    handle: Handle<FileHolder>,
    offset: u64,
    buffer: Vec<u8>,
) -> Result<()> {
    let holder = state.files.get(handle);
    let mut file = holder.file.lock().await;

    let local_branch = holder
        .local_branch
        .as_ref()
        .ok_or(ouisync_lib::Error::PermissionDenied)?
        .clone();

    file.seek(SeekFrom::Start(offset)).await?;
    file.fork(local_branch).await?;
    file.write(&buffer).await?;

    Ok(())
}

/// Truncate the file to `len` bytes.
pub(crate) async fn truncate(
    state: &ServerState,
    handle: Handle<FileHolder>,
    len: u64,
) -> Result<()> {
    let holder = state.files.get(handle);

    let mut file = holder.file.lock().await;

    let local_branch = holder
        .local_branch
        .as_ref()
        .ok_or(ouisync_lib::Error::PermissionDenied)?
        .clone();

    file.fork(local_branch).await?;
    file.truncate(len).await?;

    Ok(())
}

/// Retrieve the size of the file in bytes.
pub(crate) async fn len(state: &ServerState, handle: Handle<FileHolder>) -> u64 {
    state.files.get(handle).file.lock().await.len()
}

/// Copy the file contents into the provided raw file descriptor.
///
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

    let session = session.get();
    let port_sender = session.port_sender;

    let src = session.state.files.get(handle);
    let mut dst = fs::File::from_raw_fd(fd);

    session.runtime.spawn(async move {
        let mut src = src.file.lock().await;
        let result = src.copy_to_writer(&mut dst).await.map_err(Error::from);

        port_sender.send_result(port, result);
    });
}
