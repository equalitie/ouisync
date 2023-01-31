use crate::state::ServerState;
use crate::{
    registry::Handle,
    repository::{entry_type_to_num, RepositoryHolder},
    session::SessionHandle,
    utils::{self, Port},
};
use camino::Utf8PathBuf;
use ouisync_lib::Result;
use serde::Serialize;
use std::os::raw::c_char;

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
    path: String,
) -> Result<()> {
    let path = Utf8PathBuf::from(path);
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
    path: String,
) -> Result<Directory> {
    let path = Utf8PathBuf::from(path);
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

/// Removes the directory at the given path from the repository. The directory must be empty.
#[no_mangle]
pub unsafe extern "C" fn directory_remove(
    session: SessionHandle,
    repo: Handle<RepositoryHolder>,
    path: *const c_char,
    port: Port<Result<()>>,
) {
    session.get().with(port, |ctx| {
        let repo = ctx.state().repositories.get(repo);
        let path = utils::ptr_to_path_buf(path)?;

        ctx.spawn(async move { repo.repository.remove_entry(path).await })
    })
}

/// Removes the directory at the given path including its content from the repository.
#[no_mangle]
pub unsafe extern "C" fn directory_remove_recursively(
    session: SessionHandle,
    repo: Handle<RepositoryHolder>,
    path: *const c_char,
    port: Port<Result<()>>,
) {
    session.get().with(port, |ctx| {
        let repo = ctx.state().repositories.get(repo);
        let path = utils::ptr_to_path_buf(path)?;

        ctx.spawn(async move { repo.repository.remove_entry_recursively(path).await })
    })
}
