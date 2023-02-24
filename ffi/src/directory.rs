use crate::state::ServerState;
use crate::{
    error::Result,
    registry::Handle,
    repository::{entry_type_to_num, RepositoryHolder},
};
use camino::Utf8PathBuf;
use serde::Serialize;

// Currently this is only a read-only snapshot of a directory.
#[derive(Serialize)]
#[serde(transparent)]
pub struct Directory(Vec<DirEntry>);

#[derive(Clone, Serialize)]
pub(crate) struct DirEntry {
    pub name: String,
    pub entry_type: u8,
}

pub(crate) async fn create(
    state: &ServerState,
    repo: Handle<RepositoryHolder>,
    path: Utf8PathBuf,
) -> Result<()> {
    state
        .repositories
        .get(repo)
        .repository
        .create_directory(path)
        .await?;
    Ok(())
}

pub(crate) async fn open(
    state: &ServerState,
    repo: Handle<RepositoryHolder>,
    path: Utf8PathBuf,
) -> Result<Directory> {
    let repo = state.repositories.get(repo);

    let dir = repo.repository.open_directory(path).await?;
    let entries = dir
        .entries()
        .map(|entry| DirEntry {
            name: entry.unique_name().into_owned(),
            entry_type: entry_type_to_num(entry.entry_type()),
        })
        .collect();

    Ok(Directory(entries))
}

/// Removes the directory at the given path from the repository. If `recursive` is true it removes
/// also the contents, otherwise the directory must be empty.
pub(crate) async fn remove(
    state: &ServerState,
    repo: Handle<RepositoryHolder>,
    path: Utf8PathBuf,
    recursive: bool,
) -> Result<()> {
    let repo = &state.repositories.get(repo).repository;

    if recursive {
        repo.remove_entry_recursively(path).await?
    } else {
        repo.remove_entry(path).await?
    }

    Ok(())
}
