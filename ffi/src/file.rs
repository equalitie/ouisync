use crate::{registry::Handle, repository::RepositoryHolder, state::State};
use camino::Utf8PathBuf;
use ouisync_bridge::error::{Error, Result};
use ouisync_lib::{deadlock::AsyncMutex, Branch, File};
use std::{convert::TryInto, io::SeekFrom};

pub struct FileHolder {
    pub(crate) file: AsyncMutex<File>,
    pub(crate) local_branch: Option<Branch>,
}

pub(crate) async fn open(
    state: &State,
    repo: Handle<RepositoryHolder>,
    path: Utf8PathBuf,
) -> Result<Handle<FileHolder>> {
    let repo = state.get_repository(repo);
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
    state: &State,
    repo: Handle<RepositoryHolder>,
    path: Utf8PathBuf,
) -> Result<Handle<FileHolder>> {
    let repo = state.get_repository(repo);
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
    state: &State,
    repo: Handle<RepositoryHolder>,
    path: Utf8PathBuf,
) -> Result<()> {
    state
        .get_repository(repo)
        .repository
        .remove_entry(&path)
        .await?;
    Ok(())
}

pub(crate) async fn close(state: &State, handle: Handle<FileHolder>) -> Result<()> {
    if let Some(holder) = state.files.remove(handle) {
        holder.file.lock().await.flush().await?
    }

    Ok(())
}

pub(crate) async fn flush(state: &State, handle: Handle<FileHolder>) -> Result<()> {
    state.files.get(handle).file.lock().await.flush().await?;
    Ok(())
}

/// Read at most `len` bytes from the file and returns them. The returned buffer can be shorter
/// than `len` and empty in case of EOF.
pub(crate) async fn read(
    state: &State,
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
    state: &State,
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
pub(crate) async fn truncate(state: &State, handle: Handle<FileHolder>, len: u64) -> Result<()> {
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

/// Retrieve the total size of the file in bytes.
pub(crate) async fn len(state: &State, handle: Handle<FileHolder>) -> u64 {
    state.files.get(handle).file.lock().await.len()
}

/// Retrieve the sync progress of the file.
pub(crate) async fn progress(state: &State, handle: Handle<FileHolder>) -> Result<u64> {
    Ok(state.files.get(handle).file.lock().await.progress().await?)
}
