use crate::{error::Error, repository::RepositoryHandle, state::State};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

// Currently this is only a read-only snapshot of a directory.
#[derive(Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Directory(Vec<DirEntry>);

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DirEntry {
    pub name: String,
    pub entry_type: u8,
}

pub(crate) async fn create(
    state: &State,
    repo: RepositoryHandle,
    path: Utf8PathBuf,
) -> Result<(), Error> {
    state
        .repositories
        .get(repo)?
        .repository
        .create_directory(path)
        .await?;
    Ok(())
}

pub(crate) async fn open(
    state: &State,
    repo: RepositoryHandle,
    path: Utf8PathBuf,
) -> Result<Directory, Error> {
    let repo = state.repositories.get(repo)?;

    let dir = repo.repository.open_directory(path).await?;
    let entries = dir
        .entries()
        .map(|entry| DirEntry {
            name: entry.unique_name().into_owned(),
            entry_type: entry.entry_type().into(),
        })
        .collect();

    Ok(Directory(entries))
}

pub(crate) async fn exists(
    state: &State,
    repo: RepositoryHandle,
    path: Utf8PathBuf,
) -> Result<bool, Error> {
    let repo = state.repositories.get(repo)?;
    match repo.repository.open_directory(path).await {
        Ok(_) => Ok(true),
        Err(ouisync_lib::Error::EntryNotFound) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Removes the directory at the given path from the repository. If `recursive` is true it removes
/// also the contents, otherwise the directory must be empty.
pub(crate) async fn remove(
    state: &State,
    repo: RepositoryHandle,
    path: Utf8PathBuf,
    recursive: bool,
) -> Result<(), Error> {
    let repo = &state.repositories.get(repo)?.repository;

    if recursive {
        repo.remove_entry_recursively(path).await?
    } else {
        repo.remove_entry(path).await?
    }

    Ok(())
}
