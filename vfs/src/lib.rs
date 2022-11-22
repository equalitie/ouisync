mod entry_map;
mod inode;
mod open_flags;
mod utils;

#[cfg(test)]
mod tests;

use self::{
    entry_map::{EntryMap, FileHandle},
    inode::{Inode, InodeMap, InodeView, Representation},
    open_flags::OpenFlags,
    utils::{FormatOptionScope, MaybeOwnedMut},
};
use fuser::{
    BackgroundSession, FileAttr, FileType, KernelConfig, MountOption, ReplyAttr, ReplyCreate,
    ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use ouisync_lib::{
    DebugPrinter, EntryType, Error, File, JointDirectory, JointEntry, JointEntryRef,
    MissingVersionStrategy, Repository, Result,
};
use std::{
    convert::TryInto,
    ffi::OsStr,
    fmt,
    io::{self, SeekFrom},
    os::raw::c_int,
    panic::{self, AssertUnwindSafe},
    path::Path,
    time::{Duration, SystemTime},
};
use tracing::{instrument, Span};

// Name of the filesystem.
const FS_NAME: &str = "ouisync";

// NOTE: this is the unix implementation of virtual filesystem which is backed by fuse. Eventually
// there will be one for windows as well with the same public API but backed (probably) by
// [dokan](https://github.com/dokan-dev/dokan-rust).

/// Mount `repository` under the given directory. Spawns the filesystem handler in a background
/// thread and immediatelly returns. The returned `MountGuard` unmouts the repository on drop.
pub fn mount(
    runtime_handle: tokio::runtime::Handle,
    repository: Repository,
    mount_point: impl AsRef<Path>,
) -> Result<MountGuard, io::Error> {
    let session = fuser::spawn_mount2(
        VirtualFilesystem::new(runtime_handle, repository),
        mount_point,
        &[MountOption::FSName(FS_NAME.into())],
    )?;
    Ok(MountGuard(Some(session)))
}

/// Unmounts the virtual filesystem when dropped.
pub struct MountGuard(Option<BackgroundSession>);

impl Drop for MountGuard {
    fn drop(&mut self) {
        // Joining the fuse session on drop prevents the following failure:
        //
        // 1. A filesystem is mounted inside an async task which is ran using `block_on`
        // 2. Some time later the task completes and begins to drop
        // 3. Some other process accesses the mounted filesystem
        // 4. As part of the task being dropped, the `BackgroundSession` is dropped too which
        //    unmounts the filesystem, but does not join the background thread
        // 5. The async task finishes dropping
        // 6. The async runtime itself begins to shutdown
        // 7. The background thread begins to process the operations triggered in step 3 and as
        //    part of this processing it tries to access the async runtime (for example, to
        //    schedule a timer). The runtime is in the process of shutting down however, and so
        //    it panics.
        //
        // By joining the session here, we modify step 4 to also join the background thread which
        // ensures any queued operations on the filesystem are completed before the async runtime
        // shuts down, avoiding the panic.
        if let Some(session) = self.0.take() {
            // HACK: `BackgroundSession::join` currently panics if the background thread returns
            // an error. We don't care about that error (we are shutting down the filesystem
            // anyway), so it should be ok to just suppress the panic.
            if panic::catch_unwind(AssertUnwindSafe(move || session.join())).is_err() {
                tracing::error!("panic in BackgroundSession::join");
            }
        }
    }
}

// time-to-live for some fuse reply types.
// TODO: find out what is this for and whether 0 is OK.
const TTL: Duration = Duration::from_secs(0);

// https://libfuse.github.io/doxygen/fuse__common_8h.html#a4c81f2838716f43fe493a61c87a62816
const FUSE_CAP_ATOMIC_O_TRUNC: u32 = 8; // 0b0001

// Convenience macro that unwraps the result or reports its error in the given reply and
// returns.
macro_rules! try_request {
    ($result:expr, $reply:expr) => {
        match $result {
            Ok(value) => value,
            Err(error) => {
                $reply.error(to_error_code(&error));
                return;
            }
        }
    };
}

macro_rules! record_fmt {
    ($name:expr, $($args:tt)*) => {{
        Span::current().record($name, &format_args!($($args)*));
    }}
}

struct VirtualFilesystem {
    rt: tokio::runtime::Handle,
    inner: Inner,
}

impl VirtualFilesystem {
    fn new(runtime_handle: tokio::runtime::Handle, repository: Repository) -> Self {
        Self {
            rt: runtime_handle,
            inner: Inner {
                repository,
                inodes: InodeMap::new(),
                entries: EntryMap::default(),
            },
        }
    }
}

impl fuser::Filesystem for VirtualFilesystem {
    fn init(&mut self, _req: &Request, config: &mut KernelConfig) -> Result<(), c_int> {
        tracing::debug!(?config, "init");

        // Enable open with truncate
        if config.add_capabilities(FUSE_CAP_ATOMIC_O_TRUNC).is_err() {
            tracing::error!("required fuse capability FUSE_CAP_ATOMIC_O_TRUNC not supported");
            return Err(libc::ENOSYS);
        }

        Ok(())
    }

    fn destroy(&mut self) {
        self.rt.block_on(self.inner.repository.close());
    }

    fn lookup(&mut self, _req: &Request, parent: Inode, name: &OsStr, reply: ReplyEntry) {
        let attr = try_request!(self.rt.block_on(self.inner.lookup(parent, name)), reply);
        reply.entry(&TTL, &attr, 0)
    }

    // NOTE: This should be called for every `lookup` but also for `mkdir`, `mknod`, 'symlink`,
    // `link` and `create` (any request that uses `ReplyEntry` or `ReplyCreate`). It is *not*
    // called for the entries in `readdir` though (see the comment in that function for more
    // details).
    fn forget(&mut self, _req: &Request, inode: Inode, lookups: u64) {
        self.inner.forget(inode, lookups)
    }

    fn getattr(&mut self, _req: &Request, inode: Inode, reply: ReplyAttr) {
        let attr = try_request!(self.rt.block_on(self.inner.getattr(inode)), reply);
        reply.attr(&TTL, &attr)
    }

    fn setattr(
        &mut self,
        _req: &Request,
        inode: Inode,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        ctime: Option<SystemTime>,
        fh: Option<u64>,
        crtime: Option<SystemTime>,
        chgtime: Option<SystemTime>,
        bkuptime: Option<SystemTime>,
        flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let attr = try_request!(
            self.rt.block_on(self.inner.setattr(
                inode, mode, uid, gid, size, atime, mtime, ctime, fh, crtime, chgtime, bkuptime,
                flags,
            )),
            reply
        );
        reply.attr(&TTL, &attr);
    }

    fn opendir(&mut self, _req: &Request, inode: Inode, flags: i32, reply: ReplyOpen) {
        let handle = try_request!(
            self.rt
                .block_on(self.inner.opendir(inode, OpenFlags::from(flags))),
            reply
        );
        // TODO: what about `flags`?
        reply.opened(handle, 0);
    }

    fn releasedir(
        &mut self,
        _req: &Request,
        inode: Inode,
        handle: FileHandle,
        flags: i32,
        reply: ReplyEmpty,
    ) {
        try_request!(
            self.inner.releasedir(inode, handle, OpenFlags::from(flags)),
            reply
        );
        reply.ok();
    }

    fn readdir(
        &mut self,
        _req: &Request,
        inode: Inode,
        handle: FileHandle,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        try_request!(
            self.rt
                .block_on(self.inner.readdir(inode, handle, offset, &mut reply)),
            reply
        );
        reply.ok();
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        parent: Inode,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        let attr = try_request!(
            self.rt
                .block_on(self.inner.mkdir(parent, name, mode, umask)),
            reply
        );
        reply.entry(&TTL, &attr, 0);
    }

    fn rmdir(&mut self, _req: &Request, parent: Inode, name: &OsStr, reply: ReplyEmpty) {
        try_request!(self.rt.block_on(self.inner.rmdir(parent, name)), reply);
        reply.ok();
    }

    fn unlink(&mut self, _req: &Request, parent: Inode, name: &OsStr, reply: ReplyEmpty) {
        try_request!(self.rt.block_on(self.inner.unlink(parent, name)), reply);
        reply.ok();
    }

    fn fsyncdir(
        &mut self,
        _req: &Request<'_>,
        inode: u64,
        handle: u64,
        datasync: bool,
        reply: ReplyEmpty,
    ) {
        try_request!(
            self.rt
                .block_on(self.inner.fsyncdir(inode, handle, datasync)),
            reply
        );
        reply.ok();
    }

    fn create(
        &mut self,
        _req: &Request,
        parent: Inode,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        let (attr, handle, flags) = try_request!(
            self.rt.block_on(
                self.inner
                    .create(parent, name, mode, umask, OpenFlags::from(flags))
            ),
            reply
        );
        reply.created(&TTL, &attr, 0, handle, flags);
    }

    fn open(&mut self, _req: &Request, inode: Inode, flags: i32, reply: ReplyOpen) {
        let (handle, flags) = try_request!(
            self.rt
                .block_on(self.inner.open(inode, OpenFlags::from(flags))),
            reply
        );
        reply.opened(handle, flags);
    }

    fn release(
        &mut self,
        _req: &Request<'_>,
        inode: Inode,
        handle: FileHandle,
        flags: i32,
        _lock_owner: Option<u64>,
        flush: bool,
        reply: ReplyEmpty,
    ) {
        try_request!(
            self.rt.block_on(
                self.inner
                    .release(inode, handle, OpenFlags::from(flags), flush)
            ),
            reply
        );
        reply.ok()
    }

    fn read(
        &mut self,
        _req: &Request,
        inode: Inode,
        handle: FileHandle,
        offset: i64,
        size: u32,
        flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        let data = try_request!(
            self.rt.block_on(
                self.inner
                    .read(inode, handle, offset, size, OpenFlags::from(flags))
            ),
            reply
        );
        reply.data(&data);
    }

    fn write(
        &mut self,
        _req: &Request<'_>,
        inode: Inode,
        handle: FileHandle,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        // TODO: what about `write_flags` and `lock_owner`?

        let size = try_request!(
            self.rt.block_on(
                self.inner
                    .write(inode, handle, offset, data, OpenFlags::from(flags))
            ),
            reply
        );
        reply.written(size);
    }

    fn flush(
        &mut self,
        _req: &Request,
        inode: Inode,
        handle: FileHandle,
        _lock_owner: u64,
        reply: ReplyEmpty,
    ) {
        try_request!(self.rt.block_on(self.inner.flush(inode, handle)), reply);
        reply.ok();
    }

    fn fsync(
        &mut self,
        _req: &Request,
        inode: Inode,
        handle: FileHandle,
        datasync: bool,
        reply: ReplyEmpty,
    ) {
        try_request!(
            self.rt.block_on(self.inner.fsync(inode, handle, datasync)),
            reply
        );
        reply.ok();
    }

    fn rename(
        &mut self,
        _req: &Request,
        src_parent: Inode,
        src_name: &OsStr,
        dst_parent: Inode,
        dst_name: &OsStr,
        flags: u32,
        reply: ReplyEmpty,
    ) {
        try_request!(
            self.rt.block_on(
                self.inner
                    .rename(src_parent, src_name, dst_parent, dst_name, flags)
            ),
            reply
        );
        reply.ok()
    }
}

struct Inner {
    repository: Repository,
    inodes: InodeMap,
    entries: EntryMap,
}

impl Inner {
    #[instrument(skip(parent, name), fields(path), err)]
    async fn lookup(&mut self, parent: Inode, name: &OsStr) -> Result<FileAttr> {
        let name = name.to_str().ok_or(Error::NonUtf8FileName)?;

        self.record_path(parent, Some(name));

        let parent_path = self.inodes.get(parent).calculate_path();
        let parent_dir = self.repository.open_directory(parent_path).await?;

        let mut conn = self.repository.db().acquire().await?;

        let entry = parent_dir.lookup_unique(name)?;
        let (len, repr) = match &entry {
            JointEntryRef::File(entry) => (
                entry.open(&mut conn).await?.len(),
                Representation::File(*entry.branch().id()),
            ),
            JointEntryRef::Directory(entry) => (
                entry
                    .open(&mut conn, MissingVersionStrategy::Skip)
                    .await?
                    .len(),
                Representation::Directory,
            ),
        };

        let inode = self.inodes.lookup(parent, entry.name(), name, repr);

        Ok(make_file_attr(inode, entry.entry_type(), len))
    }

    #[instrument(skip(inode), fields(path))]
    fn forget(&mut self, inode: Inode, lookups: u64) {
        self.record_path(inode, None);
        self.inodes.forget(inode, lookups)
    }

    #[instrument(skip(inode), fields(path), err)]
    async fn getattr(&mut self, inode: Inode) -> Result<FileAttr> {
        self.record_path(inode, None);

        let entry = self.open_entry_by_inode(self.inodes.get(inode)).await?;

        Ok(make_file_attr_for_entry(&entry, inode).await)
    }

    #[allow(clippy::too_many_arguments)]
    #[instrument(
        skip(
            inode, mode, uid, gid, size, atime, mtime, ctime, handle, crtime, chgtime, bkuptime,
            flags
        ),
        fields(path),
        err
    )]
    async fn setattr(
        &mut self,
        inode: Inode,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        ctime: Option<SystemTime>,
        handle: Option<FileHandle>,
        crtime: Option<SystemTime>,
        chgtime: Option<SystemTime>,
        bkuptime: Option<SystemTime>,
        flags: Option<u32>,
    ) -> Result<FileAttr> {
        self.record_path(inode, None);

        let local_branch = self.repository.local_branch()?;

        let mut scope = FormatOptionScope::new(", ");

        tracing::debug!(
            "attrs: {:#o}{}{}{}{:?}{:?}{:?}{}{:?}{:?}{:?}{:#x}",
            scope.add("mode=", mode),
            scope.add("uid=", uid),
            scope.add("gid=", gid),
            scope.add("size=", size),
            scope.add("atime=", atime),
            scope.add("mtime=", mtime),
            scope.add("ctime=", ctime),
            scope.add("handle=", handle),
            scope.add("crtime=", crtime),
            scope.add("chgtime=", chgtime),
            scope.add("bkuptime=", bkuptime),
            scope.add("flags=", flags)
        );

        fn check_unsupported<T>(value: Option<T>) -> Result<()> {
            if value.is_none() {
                Ok(())
            } else {
                Err(Error::OperationNotSupported)
            }
        }

        check_unsupported(mode)?;
        check_unsupported(uid)?;
        check_unsupported(gid)?;
        check_unsupported(ctime)?;
        check_unsupported(crtime)?;
        check_unsupported(chgtime)?;
        check_unsupported(bkuptime)?;
        check_unsupported(flags)?;

        // NOTE: ignoring these for now to make `touch` work
        // check_unsupported(atime)?;
        // check_unsupported(mtime)?;

        let mut file = if let Some(handle) = handle {
            MaybeOwnedMut::Borrowed(self.entries.get_file_mut(handle)?)
        } else {
            MaybeOwnedMut::Owned(self.open_file_by_inode(inode).await?)
        };

        if let Some(size) = size {
            let mut conn = self.repository.db().acquire().await?;

            file.fork(&mut conn, local_branch).await?;
            file.truncate(&mut conn, size).await?;
            file.flush(&mut conn).await?;
        }

        Ok(make_file_attr(inode, EntryType::File, file.len()))
    }

    #[instrument(skip(inode, flags), fields(path, %flags), err)]
    async fn opendir(&mut self, inode: Inode, flags: OpenFlags) -> Result<FileHandle> {
        self.record_path(inode, None);

        let dir = self.open_directory_by_inode(inode).await?;
        let handle = self.entries.insert(JointEntry::Directory(dir));

        Ok(handle)
    }

    #[instrument(skip(inode, flags), fields(path, handle, %flags), err)]
    fn releasedir(&mut self, inode: Inode, handle: FileHandle, flags: OpenFlags) -> Result<()> {
        self.record_path(inode, None);

        // TODO: what about `flags`?

        self.entries.get_directory(handle)?;
        self.entries.remove(handle);

        Ok(())
    }

    #[instrument(skip(inode, reply), fields(path, handle), err)]
    async fn readdir(
        &mut self,
        inode: Inode,
        handle: FileHandle,
        offset: i64,
        reply: &mut ReplyDirectory,
    ) -> Result<()> {
        self.record_path(inode, None);

        // Print state when the user does `ls <ouisync-mount-root>/`
        if tracing::enabled!(tracing::Level::DEBUG) && inode == 1 && offset == 0 {
            self.debug_print(DebugPrinter::new()).await;
        }

        if offset < 0 {
            return Err(Error::OffsetOutOfRange);
        }

        let parent = self.inodes.get(inode).parent();
        let dir = self.entries.get_directory(handle)?;

        // Handle . and ..
        if offset <= 0 && reply.add(inode, 1, FileType::Directory, ".") {
            return Ok(());
        }

        if offset <= 1 && parent != 0 && reply.add(parent, 2, FileType::Directory, "..") {
            return Ok(());
        }

        // Index of the first "real" entry (excluding . and ..)
        let first = if parent == 0 { 1 } else { 2 };

        for (index, entry) in dir
            .entries()
            .enumerate()
            .skip((offset as usize).saturating_sub(first))
        {
            // NOTE: According to the libfuse documentation
            // (https://libfuse.github.io/doxygen/structfuse__lowlevel__ops.html#af1ef8e59e0cb0b02dc0e406898aeaa51)
            // "Returning a directory entry from readdir() does not affect its lookup count".
            // This seems to imply that `forget` won't be called for inodes returned here. So for
            // inodes that already exist, we should probably just return them here without
            // incrementing their lookup count. It's not clear however how to handle inodes that
            // don't exists yet because allocating them would probably leave them dangling.
            // That said, based on some experiments, it doesn't seem the inodes returned here are
            // even being used (there seems to be always a separate `lookup` request for each
            // entry). To keep things simple, we currently return an invalid inode which seems to
            // work fine. If we start seeing panics due to invalid inodes we need to come up with
            // a different strategy.
            if reply.add(
                u64::MAX, // invalid inode, see above.
                (index + first + 1) as i64,
                to_file_type(entry.entry_type()),
                entry.unique_name().as_ref(),
            ) {
                break;
            }
        }

        Ok(())
    }

    #[instrument(skip(parent, name), fields(path), err)]
    async fn mkdir(
        &mut self,
        parent: Inode,
        name: &OsStr,
        mode: u32,
        umask: u32,
    ) -> Result<FileAttr> {
        record_fmt!("mode", "{:#o}", mode);
        record_fmt!("umask", "{:#o}", umask);

        let name = name.to_str().ok_or(Error::NonUtf8FileName)?;
        self.record_path(parent, Some(name));

        let path = self.inodes.get(parent).calculate_path().join(name);
        let dir = self.repository.create_directory(path).await?;

        let inode = self
            .inodes
            .lookup(parent, name, name, Representation::Directory);
        let len = dir.len();

        Ok(make_file_attr(inode, EntryType::Directory, len))
    }

    #[instrument(skip(parent, name), fields(path), err)]
    async fn rmdir(&mut self, parent: Inode, name: &OsStr) -> Result<()> {
        let name = name.to_str().ok_or(Error::NonUtf8FileName)?;
        self.record_path(parent, Some(name));

        let parent_path = self.inodes.get(parent).calculate_path();
        self.repository.remove_entry(parent_path.join(name)).await
    }

    #[instrument(skip(inode), fields(path), err)]
    async fn fsyncdir(&mut self, inode: Inode, handle: FileHandle, datasync: bool) -> Result<()> {
        self.record_path(inode, None);

        // All directory operations are immediatelly synced, so there is nothing to do here.

        Ok(())
    }

    #[instrument(skip(parent, name, flags), fields(path, %flags), err)]
    async fn create(
        &mut self,
        parent: Inode,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: OpenFlags,
    ) -> Result<(FileAttr, FileHandle, u32)> {
        record_fmt!("mode", "{:#o}", mode);
        record_fmt!("umask", "{:#o}", umask);

        let name = name.to_str().ok_or(Error::NonUtf8FileName)?;
        self.record_path(parent, Some(name));

        let path = self.inodes.get(parent).calculate_path().join(name);
        let mut file = self.repository.create_file(&path).await?;
        let mut conn = self.repository.db().acquire().await?;
        file.flush(&mut conn).await?;

        let branch_id = *file.branch().id();
        let entry = JointEntry::File(file);
        let inode = self
            .inodes
            .lookup(parent, name, name, Representation::File(branch_id));
        let attr = make_file_attr_for_entry(&entry, inode).await;
        let handle = self.entries.insert(entry);

        Ok((attr, handle, 0))
    }

    #[instrument(skip(inode, flags), fields(path, %flags), err)]
    async fn open(&mut self, inode: Inode, flags: OpenFlags) -> Result<(FileHandle, u32)> {
        self.record_path(inode, None);

        let mut file = self.open_file_by_inode(inode).await?;

        if flags.contains(libc::O_TRUNC) {
            let local_branch = self.repository.local_branch()?;
            let mut conn = self.repository.db().acquire().await?;

            file.fork(&mut conn, local_branch).await?;
            file.truncate(&mut conn, 0).await?;
            file.flush(&mut conn).await?;
        }

        // TODO: what about other flags (parameter)?

        let handle = self.entries.insert(JointEntry::File(file));

        // TODO: what about flags (return value)?

        Ok((handle, 0))
    }

    #[instrument(skip(inode, flags), fields(path, %flags), err)]
    async fn release(
        &mut self,
        inode: Inode,
        handle: FileHandle,
        flags: OpenFlags,
        flush: bool,
    ) -> Result<()> {
        self.record_path(inode, None);

        // TODO: what about `flags`?
        let file = self.entries.get_file_mut(handle)?;

        if flush {
            let mut conn = self.repository.db().acquire().await?;
            file.flush(&mut conn).await?;
        }

        self.entries.remove(handle);

        Ok(())
    }

    #[instrument(skip(inode, flags), fields(path, %flags), err)]
    async fn read(
        &mut self,
        inode: Inode,
        handle: FileHandle,
        offset: i64,
        size: u32,
        flags: OpenFlags,
    ) -> Result<Vec<u8>> {
        self.record_path(inode, None);

        // TODO: what about flags?

        let mut conn = self.repository.db().acquire().await?;

        let file = self.entries.get_file_mut(handle)?;

        let offset: u64 = offset.try_into().map_err(|_| Error::OffsetOutOfRange)?;
        file.seek(&mut conn, SeekFrom::Start(offset)).await?;

        // TODO: consider reusing these buffers
        let mut buffer = vec![0; size as usize];
        let len = file.read(&mut conn, &mut buffer).await?;
        buffer.truncate(len);

        Ok(buffer)
    }

    #[instrument(
        skip(inode, data, flags),
        fields(path, data.len = data.len(), %flags),
        err
    )]
    async fn write(
        &mut self,
        inode: Inode,
        handle: FileHandle,
        offset: i64,
        data: &[u8],
        flags: OpenFlags,
    ) -> Result<u32> {
        self.record_path(inode, None);

        let offset: u64 = offset.try_into().map_err(|_| Error::OffsetOutOfRange)?;
        let local_branch = self.repository.local_branch()?;
        let mut conn = self.repository.db().acquire().await?;

        let file = self.entries.get_file_mut(handle)?;
        file.seek(&mut conn, SeekFrom::Start(offset)).await?;
        file.fork(&mut conn, local_branch).await?;
        file.write(&mut conn, data).await?;

        Ok(data.len().try_into().unwrap_or(u32::MAX))
    }

    #[instrument(skip(inode), fields(path), err)]
    async fn flush(&mut self, inode: Inode, handle: FileHandle) -> Result<()> {
        self.record_path(inode, None);

        let mut conn = self.repository.db().acquire().await?;
        self.entries.get_file_mut(handle)?.flush(&mut conn).await
    }

    #[instrument(skip(inode), fields(path), err)]
    async fn fsync(&mut self, inode: Inode, handle: FileHandle, datasync: bool) -> Result<()> {
        self.record_path(inode, None);

        // TODO: what about `datasync`?
        let mut conn = self.repository.db().acquire().await?;
        self.entries.get_file_mut(handle)?.flush(&mut conn).await
    }

    #[instrument(skip(parent, name), fields(path), err)]
    async fn unlink(&mut self, parent: Inode, name: &OsStr) -> Result<()> {
        let name = name.to_str().ok_or(Error::NonUtf8FileName)?;
        self.record_path(parent, Some(name));

        let path = self.inodes.get(parent).calculate_path().join(name);
        self.repository.remove_entry(path).await?;
        Ok(())
    }

    #[instrument(
        skip(src_parent, src_name, dst_parent, dst_name),
        fields(src_path, dst_path),
        err
    )]
    async fn rename(
        &mut self,
        src_parent: Inode,
        src_name: &OsStr,
        dst_parent: Inode,
        dst_name: &OsStr,
        flags: u32,
    ) -> Result<()> {
        record_fmt!("flags", "{:#x}", flags);

        let src_name = src_name.to_str().ok_or(Error::NonUtf8FileName)?;
        record_fmt!(
            "src_path",
            "{}",
            self.inodes.path_display(src_parent, Some(src_name)),
        );

        let dst_name = dst_name.to_str().ok_or(Error::NonUtf8FileName)?;
        record_fmt!(
            "dst_path",
            "{}",
            self.inodes.path_display(dst_parent, Some(dst_name)),
        );

        let src_dir = self.inodes.get(src_parent).calculate_path();

        let dst_dir = if src_parent == dst_parent {
            // TODO: Maybe we could use something like Cow?
            src_dir.clone()
        } else {
            self.inodes.get(dst_parent).calculate_path()
        };

        self.repository
            .move_entry(src_dir, src_name, dst_dir, dst_name)
            .await
    }

    async fn open_file_by_inode(&self, inode: Inode) -> Result<File> {
        let inode = self.inodes.get(inode);
        let branch_id = inode.representation().file_version()?;
        let path = inode.calculate_path();

        self.repository.open_file_version(path, branch_id).await
    }

    async fn open_directory_by_inode(&self, inode: Inode) -> Result<JointDirectory> {
        let path = self.inodes.get(inode).calculate_path();
        self.repository.open_directory(path).await
    }

    async fn open_entry_by_inode(&self, inode: InodeView<'_>) -> Result<JointEntry> {
        let path = inode.calculate_path();

        match inode.representation() {
            Representation::Directory => Ok(JointEntry::Directory(
                self.repository.open_directory(path).await?,
            )),
            Representation::File(branch_id) => {
                let file = self.repository.open_file_version(path, branch_id).await?;
                Ok(JointEntry::File(file))
            }
        }
    }

    // For debugging, use when needed
    #[allow(dead_code)]
    async fn debug_print(&self, print: DebugPrinter) {
        print.display(&"VirtualFilesystem::Inner");
        self.entries.debug_print(print.indent()).await;
        self.repository.debug_print(print.indent()).await;
    }

    fn record_path(&self, inode: Inode, last: Option<&str>) {
        record_fmt!("path", "{}", self.inodes.path_display(inode, last))
    }
}

impl fmt::Debug for Inner {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "VirtualFilesystem::Inner {{ local_id: {} }}",
            self.repository.local_id()
        )
    }
}

async fn make_file_attr_for_entry(entry: &JointEntry, inode: Inode) -> FileAttr {
    make_file_attr(inode, entry.entry_type(), entry.len())
}

fn make_file_attr(inode: Inode, entry_type: EntryType, len: u64) -> FileAttr {
    FileAttr {
        ino: inode,
        size: len,
        blocks: 0,                      // TODO: ?
        atime: SystemTime::UNIX_EPOCH,  // TODO
        mtime: SystemTime::UNIX_EPOCH,  // TODO
        ctime: SystemTime::UNIX_EPOCH,  // TODO
        crtime: SystemTime::UNIX_EPOCH, // TODO
        kind: to_file_type(entry_type),
        perm: match entry_type {
            EntryType::File => 0o444,      // TODO
            EntryType::Directory => 0o555, // TODO
        },
        nlink: 1,
        uid: 0, // TODO
        gid: 0, // TODO
        rdev: 0,
        blksize: 0, // ?
        flags: 0,
    }
}

// TODO: consider moving this to `impl Error`
fn to_error_code(error: &Error) -> libc::c_int {
    match error {
        Error::Db(_)
        | Error::DeviceIdConfig(_)
        | Error::BlockNotFound(_)
        | Error::BlockNotReferenced
        | Error::MalformedData
        | Error::MalformedDirectory
        | Error::WrongBlockLength(_)
        | Error::InitializeLogger(_)
        | Error::InitializeRuntime(_)
        | Error::Writer(_)
        | Error::StorageVersionMismatch => libc::EIO,
        Error::EntryNotFound | Error::AmbiguousEntry => libc::ENOENT,
        Error::EntryExists => libc::EEXIST,
        Error::EntryIsFile => libc::ENOTDIR,
        Error::EntryIsDirectory => libc::EISDIR,
        Error::NonUtf8FileName => libc::EINVAL,
        Error::OffsetOutOfRange => libc::EINVAL,
        Error::PermissionDenied => libc::EACCES,
        Error::DirectoryNotEmpty => libc::ENOTEMPTY,
        Error::OperationNotSupported => libc::ENOTSUP,
        Error::RequestTimeout => libc::ETIMEDOUT,
        Error::ConcurrentWriteNotSupported => libc::EBUSY,
    }
}

fn to_file_type(entry_type: EntryType) -> FileType {
    match entry_type {
        EntryType::File => FileType::RegularFile,
        EntryType::Directory => FileType::Directory,
    }
}
