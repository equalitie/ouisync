use std::{
    fmt, io,
    path::{Path, PathBuf},
};

use crate::{protocol::RepositoryHandle, Error};
use ouisync::{Credentials, Network};
use ouisync_vfs::{MultiRepoMount, MultiRepoVFS};
use state_monitor::StateMonitor;
use tokio::fs;

use super::{load_repository, ConfigStore, RepositoryHolder, State, Store};

/// Move or rename a repository. Makes "best effort" to do it atomically, that is, if any step of
/// this operation fails, tries to revert all previous steps before returning.
pub(super) async fn invoke(
    state: &mut State,
    handle: RepositoryHandle,
    dst: &Path,
) -> Result<(), Error> {
    let mut context = Context::new(state, handle, dst)?;
    let mut undo_stack = Vec::new();

    match context.invoke(&mut undo_stack).await {
        Ok(()) => Ok(()),
        Err(error) => {
            context.undo(&mut undo_stack).await;
            Err(error)
        }
    }
}

struct Context<'a> {
    config: &'a ConfigStore,
    network: &'a Network,
    store: &'a Store,
    mounter: Option<&'a MultiRepoVFS>,
    repos_monitor: &'a StateMonitor,
    holder: &'a mut RepositoryHolder,
    dst: PathBuf,
}

impl<'a> Context<'a> {
    fn new(state: &'a mut State, handle: RepositoryHandle, dst: &Path) -> Result<Self, Error> {
        let dst = state.store.normalize_repository_path(dst)?;

        if state.repos.find_by_path(&dst).is_some() {
            return Err(Error::AlreadyExists);
        }

        let holder = state.repos.get_mut(handle).ok_or(Error::InvalidArgument)?;

        Ok(Self {
            config: &state.config,
            network: &state.network,
            store: &state.store,
            mounter: state.mounter.as_ref(),
            repos_monitor: &state.repos_monitor,
            holder,
            dst,
        })
    }

    async fn invoke(&mut self, undo_stack: &mut Vec<Action>) -> Result<(), Error> {
        // TODO: close all open files of this repo

        // 1. Unmount the repo (if mounted)
        if let Some(mounter) = self.mounter {
            mounter.remove(self.holder.short_name())?;
            undo_stack.push(Action::Unmount);
        }

        // 2. Create the dst directory
        let dst_parent = self.dst.parent().ok_or(Error::InvalidArgument)?;
        fs::create_dir_all(dst_parent).await?;
        undo_stack.push(Action::CreateDir {
            path: dst_parent.to_owned(),
        });

        // 3. Close the repo
        let credentials = self.holder.repository().credentials();
        let sync_enabled = self.holder.registration().is_some();

        self.holder.close().await?;
        undo_stack.push(Action::CloseRepository {
            credentials: credentials.clone(),
            sync_enabled,
        });

        // 4. Move the database file(s)
        for (src, dst) in ouisync::database_files(self.holder.path())
            .into_iter()
            .zip(ouisync::database_files(&self.dst))
        {
            if !fs::try_exists(&src).await? {
                continue;
            }

            move_file(&src, &dst).await?;
            undo_stack.push(Action::MoveFile { src, dst });
        }

        // 5. Remove the old parent directory
        let src_parent = self
            .holder
            .path()
            .parent()
            .ok_or(Error::InvalidArgument)?
            .to_owned();
        self.store
            .remove_empty_ancestor_dirs(self.holder.path())
            .await?;
        undo_stack.push(Action::RemoveDir { path: src_parent });

        // 6. Open the repository from its new location
        *self.holder = self
            .load_repository(&self.dst, credentials, sync_enabled)
            .await?;

        Ok(())
    }

    async fn undo(&mut self, undo_stack: &mut Vec<Action>) {
        while let Some(action) = undo_stack.pop() {
            let action_debug = format!("{:?}", action);
            action
                .undo(self)
                .await
                .inspect_err(|error| tracing::error!(?error, "failed to undo {action_debug}"))
                .ok();
        }
    }

    async fn load_repository(
        &self,
        path: &Path,
        credentials: Credentials,
        sync_enabled: bool,
    ) -> Result<RepositoryHolder, Error> {
        let holder = load_repository(
            path,
            None,
            sync_enabled,
            self.config,
            self.network,
            self.repos_monitor,
            self.mounter,
        )
        .await?;
        holder.repository().set_credentials(credentials).await?;

        Ok(holder)
    }
}

#[expect(clippy::large_enum_variant)]
enum Action {
    Unmount,
    CreateDir {
        path: PathBuf,
    },
    CloseRepository {
        credentials: Credentials,
        sync_enabled: bool,
    },
    MoveFile {
        src: PathBuf,
        dst: PathBuf,
    },
    RemoveDir {
        path: PathBuf,
    },
}

impl Action {
    async fn undo(self, context: &mut Context<'_>) -> Result<(), Error> {
        match self {
            Self::Unmount => {
                if let Some(mounter) = context.mounter {
                    mounter.insert(
                        context.holder.short_name().to_owned(),
                        context.holder.repository().clone(),
                    )?;
                }
            }
            Self::CreateDir { path } => {
                context.store.remove_empty_ancestor_dirs(&path).await?;
            }
            Self::CloseRepository {
                credentials,
                sync_enabled,
            } => {
                *context.holder = context
                    .load_repository(context.holder.path(), credentials, sync_enabled)
                    .await?;
            }
            Self::MoveFile { src, dst } => {
                move_file(&dst, &src).await?;
            }
            Self::RemoveDir { path } => {
                fs::create_dir_all(path).await?;
            }
        }

        Ok(())
    }
}

impl fmt::Debug for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unmount => f.debug_tuple("Unmount").finish(),
            Self::CreateDir { path } => f.debug_struct("CreateDir").field("path", path).finish(),
            Self::CloseRepository { .. } => {
                f.debug_struct("CloseRepository").finish_non_exhaustive()
            }
            Self::MoveFile { src, dst } => f
                .debug_struct("MoveFile")
                .field("src", src)
                .field("dst", dst)
                .finish(),
            Self::RemoveDir { path } => f.debug_struct("RemoveDir").field("path", path).finish(),
        }
    }
}

/// Move file from `src` to `dst`. If they are on the same filesystem, it does a simple rename.
/// Otherwise it copies `src` to `dst` first and then deletes `src`.
async fn move_file(src: &Path, dst: &Path) -> io::Result<()> {
    // First try rename
    match fs::rename(src, dst).await {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => (),
        Err(error) => return Err(error),
    }

    // `src` and `dst` are on different filesystems, fall back to copy + remove.
    fs::copy(src, dst).await?;
    fs::remove_file(src).await?;

    Ok(())
}
