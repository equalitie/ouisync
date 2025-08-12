use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    protocol::RepositoryHandle,
    repository::{self, RepositorySet},
    Error,
};
use ouisync::{Credentials, Network, Repository};
use ouisync_vfs::{MultiRepoMount, MultiRepoVFS};
use state_monitor::StateMonitor;
use tokio::fs;

use super::{load_repository, ConfigStore, RepositoryHolder, State, Store};

/// Move or rename a repository. Makes "best effort" to do it atomically, that is, if any step of
/// this operation fails, tries to revert all previous steps before returning.
pub(super) async fn invoke(
    state: &State,
    handle: RepositoryHandle,
    dst: &Path,
) -> Result<(), Error> {
    let context = Context::new(state, handle, dst)?;
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
    repos: &'a RepositorySet,
    mounter: Option<Arc<MultiRepoVFS>>,
    repos_monitor: &'a StateMonitor,
    handle: RepositoryHandle,
    sync_enabled: bool,
    src: PathBuf,
    dst: PathBuf,
}

impl<'a> Context<'a> {
    fn new(state: &'a State, handle: RepositoryHandle, dst: &Path) -> Result<Self, Error> {
        let dst = state.store.normalize_repository_path(dst)?;

        if state.repos.find_by_path(&dst).is_some() {
            return Err(Error::AlreadyExists);
        }

        let (sync_enabled, src) = state
            .repos
            .with(handle, |holder| {
                (holder.is_sync_enabled(), holder.path().to_owned())
            })
            .ok_or(Error::InvalidArgument)?;

        // Grab mounter only if mounting is supported, enabled and the repo is currently mounted.
        let mounter = state
            .mounter
            .lock()
            .unwrap()
            .as_ref()
            .filter(|mounter| mounter.mount_point(repository::short_name(&src)).is_some())
            .cloned();

        Ok(Self {
            config: &state.config,
            network: &state.network,
            store: &state.store,
            repos: &state.repos,
            mounter,
            repos_monitor: &state.repos_monitor,
            handle,
            sync_enabled,
            src,
            dst,
        })
    }

    async fn invoke(&self, undo_stack: &mut Vec<Action>) -> Result<(), Error> {
        // TODO: close all open files of this repo

        // 1. Unmount the repo (if mounted)
        if let Some(mounter) = &self.mounter {
            mounter.remove(repository::short_name(&self.src))?;
            undo_stack.push(Action::Unmount);
        }

        // 2. Create the dst directory
        let dst_parent = self.dst.parent().ok_or(Error::InvalidArgument)?;
        fs::create_dir_all(dst_parent).await?;
        undo_stack.push(Action::CreateDir {
            path: dst_parent.to_owned(),
        });

        // 3. Close the repo
        let repo = self.get_repository();
        let credentials = repo.credentials();
        let sync_enabled = self.sync_enabled;

        repo.close().await?;
        undo_stack.push(Action::CloseRepository {
            credentials: credentials.clone(),
            sync_enabled,
        });

        // 4. Move the database file(s)
        for (src, dst) in ouisync::database_files(&self.src)
            .into_iter()
            .zip(ouisync::database_files(&self.dst))
        {
            if !fs::try_exists(&src).await? {
                continue;
            }

            fs_util::safe_move(&src, &dst).await?;
            undo_stack.push(Action::MoveFile { src, dst });
        }

        // 5. Remove the old parent directory
        let src_parent = self.src.parent().ok_or(Error::InvalidArgument)?.to_owned();
        self.store.remove_empty_ancestor_dirs(&self.src).await?;
        undo_stack.push(Action::RemoveDir { path: src_parent });

        // 6. Open the repository from its new location
        let holder = self
            .load_repository(&self.dst, credentials, sync_enabled)
            .await?;
        self.repos.replace(self.handle, holder);
        undo_stack.push(Action::Open);

        // 7. Remount the repository
        self.mount_repository(&self.dst)?;

        Ok(())
    }

    async fn undo(&self, undo_stack: &mut Vec<Action>) {
        while let Some(action) = undo_stack.pop() {
            let action_debug = format!("{action:?}");
            action
                .undo(self)
                .await
                .inspect_err(|error| tracing::error!(?error, "failed to undo {action_debug}"))
                .ok();
        }
    }

    fn get_repository(&self) -> Arc<Repository> {
        // unwrap is ok because the handle validity is checked at construction.
        self.repos.get_repository(self.handle).unwrap()
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
            None,
        )
        .await?;

        holder.repository().set_credentials(credentials).await?;

        Ok(holder)
    }

    fn mount_repository(&self, path: &Path) -> Result<(), Error> {
        if let Some(mounter) = &self.mounter {
            mounter.insert(
                repository::short_name(path).to_owned(),
                self.get_repository(),
            )?;
        }

        Ok(())
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
    Open,
}

impl Action {
    async fn undo(self, context: &Context<'_>) -> Result<(), Error> {
        match self {
            Self::Unmount => context.mount_repository(&context.src)?,
            Self::CreateDir { path } => {
                context.store.remove_empty_ancestor_dirs(&path).await?;
            }
            Self::CloseRepository {
                credentials,
                sync_enabled,
            } => {
                let holder = context
                    .load_repository(&context.src, credentials, sync_enabled)
                    .await?;
                context.repos.replace(context.handle, holder);
            }
            Self::MoveFile { src, dst } => {
                fs_util::safe_move(&dst, &src).await?;
            }
            Self::RemoveDir { path } => {
                fs::create_dir_all(path).await?;
            }
            Self::Open => {
                context.get_repository().close().await?;
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
            Self::CloseRepository { sync_enabled, .. } => f
                .debug_struct("CloseRepository")
                .field("sync_enabled", sync_enabled)
                .finish_non_exhaustive(),
            Self::MoveFile { src, dst } => f
                .debug_struct("MoveFile")
                .field("src", src)
                .field("dst", dst)
                .finish(),
            Self::RemoveDir { path } => f.debug_struct("RemoveDir").field("path", path).finish(),
            Self::Open => f.debug_struct("Open").finish(),
        }
    }
}
