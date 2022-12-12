use super::Shared;
use crate::{
    branch::Branch,
    directory::MissingBlockStrategy,
    error::{Error, Result},
    event::Payload,
    event::{EventScope, IgnoreScopeReceiver},
    joint_directory::JointDirectory,
};
use std::sync::Arc;
use tokio::{
    select,
    sync::{broadcast::error::RecvError, mpsc, oneshot},
};
use tracing::instrument;

/// Background worker to perform various jobs on the repository:
/// - merge remote branches into the local one
/// - remove outdated branches and snapshots
/// - remove unreachable blocks
/// - find missing blocks
pub(super) struct Worker {
    inner: Inner,
    command_rx: mpsc::Receiver<Command>,
    abort_rx: oneshot::Receiver<()>,
}

impl Worker {
    pub fn new(shared: Arc<Shared>, local_branch: Option<Branch>) -> (Self, WorkerHandle) {
        let (command_tx, command_rx) = mpsc::channel(1);
        let (abort_tx, abort_rx) = oneshot::channel();

        let inner = Inner {
            shared,
            local_branch,
            event_scope: EventScope::new(),
        };

        let worker = Self {
            inner,
            command_rx,
            abort_rx,
        };
        let handle = WorkerHandle {
            command_tx,
            _abort_tx: abort_tx,
        };

        (worker, handle)
    }

    pub async fn run(self) {
        select! {
            _ = self.inner.run(self.command_rx) => (),
            _ = self.abort_rx => (),
        }
    }
}

/// Handle to interact with the worker. Aborts the worker task when dropped.
pub(super) struct WorkerHandle {
    command_tx: mpsc::Sender<Command>,
    _abort_tx: oneshot::Sender<()>,
}

impl WorkerHandle {
    pub async fn work(&self) -> Result<()> {
        self.oneshot(Command::Work).await
    }

    pub async fn shutdown(&self) {
        let (result_tx, result_rx) = oneshot::channel();
        self.command_tx
            .send(Command::Shutdown(result_tx))
            .await
            .unwrap_or(());
        result_rx.await.unwrap_or(())
    }

    async fn oneshot<F>(&self, command_fn: F) -> Result<()>
    where
        F: FnOnce(oneshot::Sender<Result<()>>) -> Command,
    {
        let (result_tx, result_rx) = oneshot::channel();
        self.command_tx
            .send(command_fn(result_tx))
            .await
            .unwrap_or(());

        // When this returns error it means the worker has been terminated which we treat as
        // success, for simplicity.
        result_rx.await.unwrap_or(Ok(()))
    }
}

enum Command {
    Work(oneshot::Sender<Result<()>>),
    Shutdown(oneshot::Sender<()>),
}

struct Inner {
    shared: Arc<Shared>,
    local_branch: Option<Branch>,
    event_scope: EventScope,
}

impl Inner {
    async fn run(self, mut command_rx: mpsc::Receiver<Command>) {
        let mut event_rx =
            IgnoreScopeReceiver::new(self.shared.store.index.subscribe(), self.event_scope);

        enum State {
            Working,
            Waiting,
            Terminated,
        }

        let mut state = State::Working;

        loop {
            match state {
                State::Working => {
                    state = State::Waiting;

                    let work = async {
                        self.work(ErrorHandling::Ignore).await.ok();
                    };

                    let wait = async {
                        loop {
                            match event_rx.recv().await {
                                Ok(Payload::BranchChanged(_)) => {
                                    // On `BranchChanged`, interrupt the current job and
                                    // immediately start a new one.
                                    tracing::trace!("job interrupted");
                                    state = State::Working;
                                    break;
                                }
                                Ok(Payload::BlockReceived { .. } | Payload::FileClosed)
                                | Err(RecvError::Lagged(_)) => {
                                    // On any other event, let the current job run to completion
                                    // and then start a new one.
                                    state = State::Working;
                                }
                                Err(RecvError::Closed) => {
                                    state = State::Terminated;
                                    break;
                                }
                            }
                        }
                    };

                    select! {
                        _ = work => (),
                        _ = wait => (),
                    }
                }
                State::Waiting => {
                    state = select! {
                        event = event_rx.recv() => {
                            match event {
                                Ok(_) | Err(RecvError::Lagged(_)) => State::Working,
                                Err(RecvError::Closed) => State::Terminated,
                            }
                        }
                        command = command_rx.recv() => {
                            if let Some(command) = command {
                                if self.handle_command(command).await {
                                    State::Waiting
                                } else {
                                    State::Terminated
                                }
                            } else {
                                State::Terminated
                            }
                        }
                    }
                }
                State::Terminated => break,
            }
        }
    }

    async fn handle_command(&self, command: Command) -> bool {
        match command {
            Command::Work(result_tx) => {
                result_tx
                    .send(self.work(ErrorHandling::Return).await)
                    .unwrap_or(());
                true
            }
            Command::Shutdown(result_tx) => {
                result_tx.send(()).unwrap_or(());
                false
            }
        }
    }

    async fn work(&self, error_handling: ErrorHandling) -> Result<()> {
        tracing::trace!("job started");

        // Merge
        if let Some(local_branch) = &self.local_branch {
            let result = self
                .event_scope
                .apply(merge::run(&self.shared, local_branch))
                .await;
            tracing::trace!(?result, "merge completed");

            error_handling.apply(result)?;
        }

        // Prune outdated branches and snapshots
        let result = prune::run(&self.shared).await;
        tracing::trace!(?result, "prune completed");

        error_handling.apply(result)?;

        // Scan for missing and unreachable blocks
        if self.shared.secrets.can_read() {
            let result = scan::run(&self.shared, scan::Mode::RequireAndCollect).await;
            tracing::trace!(?result, "scan completed");

            error_handling.apply(result)?;
        }

        tracing::trace!("job completed");

        Ok(())
    }
}

#[derive(Copy, Clone)]
enum ErrorHandling {
    Return,
    Ignore,
}

impl ErrorHandling {
    fn apply(self, result: Result<()>) -> Result<()> {
        match (self, result) {
            (_, Ok(())) | (Self::Ignore, Err(_)) => Ok(()),
            (Self::Return, Err(error)) => Err(error),
        }
    }
}

/// Merge remote branches into the local one.
mod merge {
    use super::*;

    #[instrument(skip_all, err(Debug))]
    pub(super) async fn run(shared: &Shared, local_branch: &Branch) -> Result<()> {
        let branches = shared.load_branches().await?;
        let mut roots = Vec::with_capacity(branches.len());

        for branch in branches {
            match branch.open_root(MissingBlockStrategy::Fail).await {
                Ok(dir) => roots.push(dir),
                Err(Error::EntryNotFound | Error::BlockNotFound(_)) => continue,
                Err(error) => return Err(error),
            }
        }

        JointDirectory::new(Some(local_branch.clone()), roots)
            .merge()
            .await?;

        Ok(())
    }
}

/// Remove outdated branches and snapshots.
mod prune {
    use super::*;
    use crate::joint_directory::versioned;

    #[instrument(skip_all, err(Debug))]
    pub(super) async fn run(shared: &Shared) -> Result<()> {
        let all = shared.store.index.load_snapshots().await?;
        let (uptodate, outdated): (Vec<_>, Vec<_>) =
            versioned::partition(all, Some(&shared.this_writer_id));

        // Remove outdated branches
        for snapshot in outdated {
            // Never remove local branch
            if snapshot.branch_id() == &shared.this_writer_id {
                continue;
            }

            match shared.inflate(snapshot.to_branch_data()) {
                Ok(branch) => {
                    // Don't remove branches that are in use. We get notified when they stop being
                    // used so we can try again.
                    if branch.is_any_file_open() {
                        tracing::trace!(id = ?branch.id(), "not removing outdated branch - in use");
                        continue;
                    }
                }
                Err(Error::PermissionDenied) => {
                    // This means we don't have read access. In that case no blobs can be open
                    // anyway so it's safe to ignore this.
                }
                Err(error) => return Err(error),
            }

            tracing::trace!(id = ?snapshot.branch_id(), "removing outdated branch");

            let mut tx = shared.store.db().begin().await?;
            snapshot.remove_all_older(&mut tx).await?;
            snapshot.remove(&mut tx).await?;
            tx.commit().await?;
        }

        // Remove outdated snapshots.
        for snapshot in uptodate {
            snapshot.prune(shared.store.db()).await?;
        }

        Ok(())
    }
}

/// Scan repository blocks to identify which ones are missing and which are no longer needed.
mod scan {
    use super::*;
    use crate::{
        blob::BlockIds,
        blob_id::BlobId,
        block,
        joint_directory::{JointEntryRef, MissingVersionStrategy},
    };
    use async_recursion::async_recursion;

    #[derive(Copy, Clone, Debug)]
    pub(super) enum Mode {
        // only require missing blocks
        Require,
        // only collect unreachable blocks
        Collect,
        // require missing blocks and collect unreachable blocks
        RequireAndCollect,
    }

    impl Mode {
        fn intersect(self, other: Self) -> Option<Self> {
            match (self, other) {
                (Self::Require, Self::Require)
                | (Self::Require, Self::RequireAndCollect)
                | (Self::RequireAndCollect, Self::Require) => Some(Self::Require),
                (Self::Collect, Self::Collect)
                | (Self::Collect, Self::RequireAndCollect)
                | (Self::RequireAndCollect, Self::Collect) => Some(Self::Collect),
                (Self::RequireAndCollect, Self::RequireAndCollect) => Some(Self::RequireAndCollect),
                (Self::Require, Self::Collect) | (Self::Collect, Self::Require) => None,
            }
        }
    }

    #[instrument(skip(shared), err(Debug))]
    pub(super) async fn run(shared: &Shared, mode: Mode) -> Result<()> {
        prepare_unreachable_blocks(shared).await?;
        let mode = traverse_root(shared, mode).await?;

        if matches!(mode, Mode::Collect | Mode::RequireAndCollect) {
            remove_unreachable_blocks(shared).await?;
        }

        Ok(())
    }

    async fn traverse_root(shared: &Shared, mut mode: Mode) -> Result<Mode> {
        let branches = shared.load_branches().await?;

        let mut versions = Vec::with_capacity(branches.len());
        let mut entries = Vec::new();

        for branch in branches {
            entries.push(BlockIds::new(branch.clone(), BlobId::ROOT));

            match branch.open_root(MissingBlockStrategy::Fail).await {
                Ok(dir) => versions.push(dir),
                Err(Error::EntryNotFound) => {
                    // `EntryNotFound` here just means this is a newly created branch with no
                    // content yet. It is safe to ignore it.
                    continue;
                }
                Err(error) => {
                    // On error, we can still proceed with finding missing blocks, but we can't
                    // proceed with garbage collection because we could incorrectly mark some
                    // blocks as unreachable due to not being able to descend into this directory.
                    mode = mode.intersect(Mode::Require).ok_or(error)?;
                }
            }
        }

        for entry in entries {
            process_blocks(shared, mode, entry).await?;
        }

        let local_branch = shared.local_branch().ok();
        traverse(shared, mode, JointDirectory::new(local_branch, versions)).await
    }

    #[async_recursion]
    async fn traverse(shared: &Shared, mut mode: Mode, dir: JointDirectory) -> Result<Mode> {
        let mut entries = Vec::new();
        let mut subdirs = Vec::new();

        // Collect the entries first, so we don't keep the directories locked while we are
        // processing the entries.
        for entry in dir.entries() {
            match entry {
                JointEntryRef::File(entry) => {
                    entries.push(BlockIds::new(
                        entry.inner().branch().clone(),
                        *entry.inner().blob_id(),
                    ));
                }
                JointEntryRef::Directory(entry) => {
                    for version in entry.versions() {
                        entries.push(BlockIds::new(version.branch().clone(), *version.blob_id()));
                    }

                    match entry
                        .open_with(MissingVersionStrategy::Fail, MissingBlockStrategy::Fail)
                        .await
                    {
                        Ok(dir) => subdirs.push(dir),
                        Err(error) => {
                            // On error, we can still proceed with finding missing blocks, but we
                            // can't proceed with garbage collection because we could incorrectly
                            // mark some blocks as unreachable due to not being able to descend
                            // into this directory.
                            mode = mode.intersect(Mode::Require).ok_or(error)?;
                        }
                    }
                }
            }
        }

        for entry in entries {
            process_blocks(shared, mode, entry).await?;
        }

        for dir in subdirs {
            mode = traverse(shared, mode, dir).await?;
        }

        Ok(mode)
    }

    async fn prepare_unreachable_blocks(shared: &Shared) -> Result<()> {
        let mut tx = shared.store.db().begin().await?;
        block::mark_all_unreachable(&mut tx).await?;
        tx.commit().await?;

        Ok(())
    }

    async fn remove_unreachable_blocks(shared: &Shared) -> Result<()> {
        let count = shared.store.remove_unreachable_blocks().await?;

        if count > 0 {
            tracing::debug!("unreachable blocks removed: {}", count);
        }

        Ok(())
    }

    async fn process_blocks(shared: &Shared, mode: Mode, mut block_ids: BlockIds) -> Result<()> {
        let mut conn = shared.store.db().acquire().await?;

        while let Some(block_id) = block_ids.next(&mut conn).await? {
            if matches!(mode, Mode::RequireAndCollect | Mode::Collect) {
                block::mark_reachable(&mut conn, &block_id).await?;
            }

            if matches!(mode, Mode::RequireAndCollect | Mode::Require) {
                shared
                    .store
                    .require_missing_block(&mut conn, block_id)
                    .await?;
            }
        }

        Ok(())
    }
}
