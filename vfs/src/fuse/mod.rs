mod entry_map;
mod flags;
mod inode;
mod multi_repo_vfs;
mod utils;

pub use multi_repo_vfs::MultiRepoVFS;

use self::{
    entry_map::{EntryMap, FileHandle},
    flags::{OpenFlags, RenameFlags},
    inode::{Inode, InodeMap, Representation},
    utils::{FormatOptionScope, MaybeOwnedMut},
};
use fuser::{
    BackgroundSession, FileAttr, FileType, KernelConfig, MountOption, ReplyAttr, ReplyCreate,
    ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use ouisync_lib::{
    AccessMode, DebugPrinter, EntryType, Error, File, JointDirectory, JointEntry, JointEntryRef,
    Repository, Result,
};
use std::{
    convert::TryInto,
    ffi::OsStr,
    io::{self, SeekFrom},
    os::raw::c_int,
    panic::{self, AssertUnwindSafe},
    path::Path,
    sync::Arc,
    time::SystemTime,
};
use tokio::time::Duration;
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
    repository: Arc<Repository>,
    mount_point: impl AsRef<Path>,
) -> Result<MountGuard, io::Error> {
    // TODO: Would be great if we could use MountOption::AutoUnmount, but the documentation
    // say it can't be used without MountOption::AllowOther or MountOption::AllowRoot. However
    // the two latter options require modifications to /etc/fuse.conf. It's not clear to me
    // whether this is a limitation of fuser or libfuse. Fuser has an open ticket for it here
    // https://github.com/cberner/fuser/issues/230
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
    fn new(runtime_handle: tokio::runtime::Handle, repository: Arc<Repository>) -> Self {
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

    fn getattr(&mut self, _req: &Request, inode: Inode, _fh: Option<u64>, reply: ReplyAttr) {
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
            self.rt.block_on(self.inner.opendir(inode, flags.into())),
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
        try_request!(self.inner.releasedir(inode, handle, flags.into()), reply);
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
        req: &Request,
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
                    .create(parent, name, mode, umask, flags.into(), req)
            ),
            reply
        );
        reply.created(&TTL, &attr, 0, handle, flags);
    }

    fn open(&mut self, _req: &Request, inode: Inode, flags: i32, reply: ReplyOpen) {
        let (handle, flags) = try_request!(
            self.rt.block_on(self.inner.open(inode, flags.into())),
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
            self.rt
                .block_on(self.inner.release(inode, handle, flags.into(), flush)),
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
            self.rt
                .block_on(self.inner.read(inode, handle, offset, size, flags.into())),
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
            self.rt
                .block_on(self.inner.write(inode, handle, offset, data, flags.into())),
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
            self.rt.block_on(self.inner.rename(
                src_parent,
                src_name,
                dst_parent,
                dst_name,
                flags.into()
            )),
            reply
        );
        reply.ok()
    }
}

struct Inner {
    repository: Arc<Repository>,
    inodes: InodeMap,
    entries: EntryMap,
}

impl Inner {
    #[instrument(skip(self, parent, name), fields(path), err(Debug))]
    async fn lookup(&mut self, parent: Inode, name: &OsStr) -> Result<FileAttr> {
        let name = name.to_str().ok_or(Error::NonUtf8FileName)?;

        self.record_path(parent, Some(name));

        let parent_path = self.inodes.get(parent).calculate_path();
        let parent_dir = self.repository.open_directory(parent_path).await?;

        let entry = parent_dir.lookup_unique(name)?;
        let (len, repr) = match &entry {
            JointEntryRef::File(entry) => (
                entry.open().await?.len(),
                Representation::File(*entry.branch().id()),
            ),
            JointEntryRef::Directory(entry) => {
                (entry.open().await?.len(), Representation::Directory)
            }
        };

        let inode = self.inodes.lookup(parent, entry.name(), name, repr);

        Ok(self.make_file_attr(inode, entry.entry_type(), len))
    }

    #[instrument(skip(self, inode), fields(path))]
    fn forget(&mut self, inode: Inode, lookups: u64) {
        self.record_path(inode, None);
        self.inodes.forget(inode, lookups)
    }

    #[instrument(skip(self, inode), fields(path), err(Debug))]
    async fn getattr(&mut self, inode: Inode) -> Result<FileAttr> {
        self.record_path(inode, None);

        let entry = self.open_entry_by_inode(inode).await?;

        Ok(self.make_file_attr_for_entry(&entry, inode))
    }

    #[allow(clippy::too_many_arguments)]
    #[instrument(
        skip(
            self, inode, mode, uid, gid, size, atime, mtime, ctime, handle, crtime, chgtime,
            bkuptime, flags
        ),
        fields(path),
        err(Debug)
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

        #[track_caller]
        fn warn_no_effect<T>(name: &str, value: Option<T>) {
            if value.is_some() {
                tracing::warn!("setting {name} has no effect");
            }
        }

        let mut entry = if let Some(handle) = handle {
            MaybeOwnedMut::Borrowed(self.entries.get_mut(handle)?)
        } else {
            MaybeOwnedMut::Owned(self.open_entry_by_inode(inode).await?)
        };

        // Setting file mode, user or group is not currently supported but if they are set to the
        // same value as what the file already has we accept it.

        if let Some(mode) = mode {
            if mode & MODE_MASK != default_mode(&self.repository, entry.entry_type()) as u32 {
                return Err(Error::OperationNotSupported);
            }
        }

        if let Some(uid) = uid {
            if uid != default_uid() {
                return Err(Error::OperationNotSupported);
            }
        }

        if let Some(gid) = gid {
            if gid != default_gid() {
                return Err(Error::OperationNotSupported);
            }
        }

        if let Some(size) = size {
            let file = entry.as_file_mut()?;

            file.fork(local_branch).await?;
            file.truncate(size)?;
            file.flush().await?;
        }

        check_unsupported(ctime)?;
        check_unsupported(crtime)?;
        check_unsupported(chgtime)?;
        check_unsupported(bkuptime)?;
        check_unsupported(flags)?;

        // Ignore these so `touch` still works.
        warn_no_effect("atime", atime);
        warn_no_effect("mtime", mtime);

        let entry_type = entry.entry_type();
        let entry_len = entry.len();

        Ok(self.make_file_attr(inode, entry_type, entry_len))
    }

    #[instrument(skip(self, inode, flags), fields(path, ?flags), err(Debug))]
    async fn opendir(&mut self, inode: Inode, flags: OpenFlags) -> Result<FileHandle> {
        self.record_path(inode, None);

        let dir = self.open_directory_by_inode(inode).await?;
        let handle = self.entries.insert(JointEntry::Directory(dir));

        Ok(handle)
    }

    #[instrument(skip(self, inode, flags), fields(path, handle, ?flags), err(Debug))]
    fn releasedir(&mut self, inode: Inode, handle: FileHandle, flags: OpenFlags) -> Result<()> {
        self.record_path(inode, None);

        // TODO: what about `flags`?

        self.entries.get_directory(handle)?;
        self.entries.remove(handle);

        Ok(())
    }

    #[instrument(skip(self, inode, reply), fields(path, handle), err(Debug))]
    async fn readdir(
        &mut self,
        inode: Inode,
        handle: FileHandle,
        offset: i64,
        reply: &mut ReplyDirectory,
    ) -> Result<()> {
        self.record_path(inode, None);

        // // Print state when the user does `ls <ouisync-mount-root>/`
        // if tracing::enabled!(tracing::Level::DEBUG) && inode == 1 && offset == 0 {
        //     self.debug_print(DebugPrinter::new()).await;
        // }

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

    #[instrument(skip(self, parent, name), fields(path), err(Debug))]
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

        Ok(self.make_file_attr(inode, EntryType::Directory, len))
    }

    #[instrument(skip(self, parent, name), fields(path), err(Debug))]
    async fn rmdir(&mut self, parent: Inode, name: &OsStr) -> Result<()> {
        let name = name.to_str().ok_or(Error::NonUtf8FileName)?;
        self.record_path(parent, Some(name));

        let parent_path = self.inodes.get(parent).calculate_path();
        self.repository.remove_entry(parent_path.join(name)).await
    }

    #[instrument(skip(self, inode), fields(path), err(Debug))]
    async fn fsyncdir(&mut self, inode: Inode, handle: FileHandle, datasync: bool) -> Result<()> {
        self.record_path(inode, None);

        // All directory operations are immediatelly synced, so there is nothing to do here.

        Ok(())
    }

    #[instrument(skip_all, fields(path, mode, umask, ?flags), err(Debug))]
    async fn create(
        &mut self,
        parent: Inode,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: OpenFlags,
        req: &Request<'_>,
    ) -> Result<(FileAttr, FileHandle, u32)> {
        record_fmt!("mode", "{:#o}", mode);
        record_fmt!("umask", "{:#o}", umask);
        record_fmt!("uid", "{}", req.uid());
        record_fmt!("gid", "{}", req.gid());

        let name = name.to_str().ok_or(Error::NonUtf8FileName)?;
        self.record_path(parent, Some(name));

        let path = self.inodes.get(parent).calculate_path().join(name);
        let mut file = self.repository.create_file(&path).await?;
        file.flush().await?;

        let branch_id = *file.branch().id();
        let entry = JointEntry::File(file);
        let inode = self
            .inodes
            .lookup(parent, name, name, Representation::File(branch_id));
        let attr = self.make_file_attr_for_entry(&entry, inode);
        let handle = self.entries.insert(entry);

        Ok((attr, handle, 0))
    }

    #[instrument(skip_all, fields(path, ?flags), err(Debug))]
    async fn open(&mut self, inode: Inode, flags: OpenFlags) -> Result<(FileHandle, u32)> {
        self.record_path(inode, None);

        let mut file = self.open_file_by_inode(inode).await?;

        if flags.contains(OpenFlags::TRUNC) {
            let local_branch = self.repository.local_branch()?;

            file.fork(local_branch).await?;
            file.truncate(0)?;
            file.flush().await?;
        }

        // TODO: what about other flags (parameter)?

        let handle = self.entries.insert(JointEntry::File(file));

        // TODO: what about flags (return value)?

        Ok((handle, 0))
    }

    #[instrument(skip(self, inode, flags), fields(path, ?flags), err(Debug))]
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
            file.flush().await?;
        }

        self.entries.remove(handle);

        Ok(())
    }

    #[instrument(skip(self, inode, flags), fields(path, ?flags), err(Debug))]
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

        let file = self.entries.get_file_mut(handle)?;

        let offset: u64 = offset.try_into().map_err(|_| Error::OffsetOutOfRange)?;
        file.seek(SeekFrom::Start(offset));

        // TODO: consider reusing these buffers
        let mut buffer = vec![0; size as usize];

        // https://libfuse.github.io/doxygen/structfuse__operations.html#a2a1c6b4ce1845de56863f8b7939501b5
        //
        //     Read should return exactly the number of bytes requested except on EOF or error...
        //
        // so we need to do `read_all` not just `read`.
        let len = file.read_all(&mut buffer).await?;
        buffer.truncate(len);

        Ok(buffer)
    }

    #[instrument(
        skip(self, inode, data, flags),
        fields(path, data.len = data.len(), ?flags),
        err(Debug)
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

        let file = self.entries.get_file_mut(handle)?;
        file.seek(SeekFrom::Start(offset));
        file.fork(local_branch).await?;

        // https://libfuse.github.io/doxygen/structfuse__operations.html#a897d1ece4b8b04c92d97b97b2dbf9768
        //
        //     Write should return exactly the number of bytes requested except on error.
        //
        // so we need to do `write_all` not just `write`.
        file.write_all(data).await?;

        Ok(data.len().try_into().unwrap_or(u32::MAX))
    }

    #[instrument(skip(self, inode), fields(path), err(Debug))]
    async fn flush(&mut self, inode: Inode, handle: FileHandle) -> Result<()> {
        self.record_path(inode, None);

        self.entries.get_file_mut(handle)?.flush().await
    }

    #[instrument(skip(self, inode), fields(path), err(Debug))]
    async fn fsync(&mut self, inode: Inode, handle: FileHandle, datasync: bool) -> Result<()> {
        self.record_path(inode, None);

        // TODO: what about `datasync`?
        self.entries.get_file_mut(handle)?.flush().await
    }

    #[instrument(skip(self, parent, name), fields(path), err(Debug))]
    async fn unlink(&mut self, parent: Inode, name: &OsStr) -> Result<()> {
        let name = name.to_str().ok_or(Error::NonUtf8FileName)?;
        self.record_path(parent, Some(name));

        let path = self.inodes.get(parent).calculate_path().join(name);
        self.repository.remove_entry(path).await?;
        Ok(())
    }

    #[instrument(
        skip(self, src_parent, src_name, dst_parent, dst_name),
        fields(src_path, dst_path),
        err(Debug)
    )]
    async fn rename(
        &mut self,
        src_parent: Inode,
        src_name: &OsStr,
        dst_parent: Inode,
        dst_name: &OsStr,
        flags: RenameFlags,
    ) -> Result<()> {
        if !flags.is_empty() {
            tracing::error!("flag(s) not supported");
            return Err(Error::OperationNotSupported);
        }

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
        let src = src_dir.join(src_name);

        let dst = if src_parent == dst_parent {
            src_dir.join(dst_name)
        } else {
            self.inodes.get(dst_parent).calculate_path().join(dst_name)
        };

        self.repository.move_entry(src, dst).await
    }

    async fn open_file_by_inode(&self, inode: Inode) -> Result<File> {
        self.open_entry_by_inode(inode).await?.into_file()
    }

    async fn open_directory_by_inode(&self, inode: Inode) -> Result<JointDirectory> {
        self.open_entry_by_inode(inode).await?.into_directory()
    }

    async fn open_entry_by_inode(&self, inode: Inode) -> Result<JointEntry> {
        let inode = self.inodes.get(inode);
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

    fn make_file_attr(&self, inode: Inode, entry_type: EntryType, len: u64) -> FileAttr {
        // From POSIX <sys/stat.h> reference:
        // https://pubs.opengroup.org/onlinepubs/009696699/basedefs/sys/stat.h.html
        //
        // > The unit for the st_blocks member of the stat structure is not defined within
        // > POSIX.1-2017. In some implementations it is 512 bytes. It may differ on a file system
        // > basis.
        //
        // TODO: Attempt to get the actual value of the system.
        const S_BLKSIZE: u64 = 512;
        use ouisync_lib::protocol::BLOCK_RECORD_SIZE;

        FileAttr {
            ino: inode,
            size: len,
            blocks: len.next_multiple_of(BLOCK_RECORD_SIZE).div_ceil(S_BLKSIZE),
            atime: SystemTime::UNIX_EPOCH,  // TODO
            mtime: SystemTime::UNIX_EPOCH,  // TODO
            ctime: SystemTime::UNIX_EPOCH,  // TODO
            crtime: SystemTime::UNIX_EPOCH, // TODO
            kind: to_file_type(entry_type),
            perm: default_mode(&self.repository, entry_type),
            nlink: 1,
            uid: default_uid(),
            gid: default_gid(),
            rdev: 0,
            // From POSIX <sys/stat.h> reference:
            //
            // > A file system-specific preferred I/O block size for this object.
            // ...
            // > There is no correlation between values of the st_blocks and st_blksize, and the
            // > f_bsize (from <sys/statvfs.h>) structure members.
            //
            // TODO: Ouisync's BLOCK_SIZE might be a good guess, but shold be benchmarked.
            blksize: 0, // ?
            flags: 0,
        }
    }

    fn make_file_attr_for_entry(&self, entry: &JointEntry, inode: Inode) -> FileAttr {
        self.make_file_attr(inode, entry.entry_type(), entry.len())
    }
}

// Returns the unix file mode used for all entries of the given type in the given repository
// (setting the mode on a per file/directory basis is currently not supported and the mode is
// determined only by the repo access mode).
fn default_mode(repo: &Repository, entry_type: EntryType) -> u16 {
    match (repo.access_mode(), entry_type) {
        (AccessMode::Write, EntryType::File) => 0o664,
        (AccessMode::Write, EntryType::Directory) => 0o775,
        (AccessMode::Read, EntryType::File) => 0o444,
        (AccessMode::Read, EntryType::Directory) => 0o555,
        (AccessMode::Blind, EntryType::File | EntryType::Directory) => 0o000,
    }
}

// Returns the user id of any entry is a repository (changing it is currently not supported).
fn default_uid() -> u32 {
    // SAFETY: all functions in the `libc` crate are marked as unsafe but many of them
    // (including this one) are actually safe to call without any preconditions.
    unsafe { libc::getuid() }
}

// Returns the group id of any entry in a repository (changing it is currently not supported).
fn default_gid() -> u32 {
    // SAFETY: all functions in the `libc` crate are marked as unsafe but many of them
    // (including this one) are actually safe to call without any preconditions.
    unsafe { libc::getgid() }
}

// When `setattr` is invoked, sometimes the `mode` argument has other bits set than the usual ones
// (user, group, other, setuid/segid). Use this mask to filter those bits out.
// TODO: find out what those extra bits are.
const MODE_MASK: u32 = 0o7777;

// TODO: consider moving this to `impl Error`
fn to_error_code(error: &Error) -> libc::c_int {
    match error {
        Error::Db(_)
        | Error::Store(_)
        | Error::MalformedData
        | Error::MalformedDirectory
        | Error::Writer(_)
        | Error::StorageVersionMismatch => libc::EIO,
        Error::EntryNotFound | Error::AmbiguousEntry => libc::ENOENT,
        Error::EntryExists => libc::EEXIST,
        Error::EntryIsFile => libc::ENOTDIR,
        Error::EntryIsDirectory => libc::EISDIR,
        Error::NonUtf8FileName | Error::InvalidArgument => libc::EINVAL,
        Error::OffsetOutOfRange => libc::EINVAL,
        Error::PermissionDenied => libc::EACCES,
        Error::DirectoryNotEmpty => libc::ENOTEMPTY,
        Error::OperationNotSupported => libc::ENOTSUP,
        Error::Locked => libc::EBUSY,
    }
}

fn to_file_type(entry_type: EntryType) -> FileType {
    match entry_type {
        EntryType::File => FileType::RegularFile,
        EntryType::Directory => FileType::Directory,
    }
}
