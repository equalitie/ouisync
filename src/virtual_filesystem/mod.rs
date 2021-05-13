mod entry_map;
mod inode;
mod open_flags;
mod utils;

#[cfg(test)]
mod tests;

use self::{
    entry_map::{EntryMap, FileHandle},
    inode::{Inode, InodeDetails, InodeMap},
    open_flags::OpenFlags,
    utils::{FormatOptionScope, MaybeOwnedMut},
};
use fuser::{
    BackgroundSession, FileAttr, FileType, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use ouisync::{Directory, Entry, EntryType, Error, File, MoveDstDirectory, Repository, Result};
use std::{
    convert::TryInto,
    ffi::OsStr,
    io::{self, SeekFrom},
    path::Path,
    time::{Duration, SystemTime},
};

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
    let session = fuser::spawn_mount(
        VirtualFilesystem::new(runtime_handle, repository),
        mount_point,
        &[],
    )?;
    Ok(MountGuard(session))
}

/// Unmounts the virtual filesystem when dropped.
pub struct MountGuard(BackgroundSession);

// time-to-live for some fuse reply types.
// TODO: find out what is this for and whether 0 is OK.
const TTL: Duration = Duration::from_secs(0);

// Convenience macro that unwraps the result or reports its error in the given reply and
// returns.
macro_rules! try_request {
    ($result:expr, $reply:expr) => {
        match $result {
            Ok(value) => value,
            Err(error) => {
                log::error!("{}", error);
                $reply.error(to_error_code(&error));
                return;
            }
        }
    };
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
    fn lookup(&mut self, _req: &Request, parent: Inode, name: &OsStr, reply: ReplyEntry) {
        let attr = try_request!(self.rt.block_on(self.inner.lookup(parent, name)), reply);
        reply.entry(&TTL, &attr, 0)
    }

    // NOTE: This should be called for every `lookup` but also for `mkdir`, `mknod`, 'symlink`,
    // `link` and `create` (any request that uses `ReplyEntry` or `ReplyCreate`). It is *not*
    // called for the entries in `readdir` though (see the comment in that function for more
    // details).
    fn forget(&mut self, _req: &Request, inode: Inode, lookups: u64) {
        log::debug!(
            "forget {} (lookups={})",
            self.inner.inodes.path_display(inode, None),
            lookups
        );
        self.inner.inodes.forget(inode, lookups)
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
        try_request!(self.inner.readdir(inode, handle, offset, &mut reply), reply);
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
    async fn lookup(&mut self, parent: Inode, name: &OsStr) -> Result<FileAttr> {
        log::debug!("lookup {}", self.inodes.path_display(parent, Some(name)));

        let parent_dir = self.open_directory_by_inode(parent).await?;
        let entry_info = parent_dir.lookup(name)?;

        let entry = entry_info.open().await?;

        let inode = self
            .inodes
            .lookup(parent, name, entry_info.locator(), entry_info.entry_type());

        Ok(make_file_attr_for_entry(&entry, inode))
    }

    async fn getattr(&mut self, inode: Inode) -> Result<FileAttr> {
        log::debug!("getattr {}", self.inodes.path_display(inode, None));

        let &InodeDetails {
            locator,
            entry_type,
            ..
        } = self.inodes.get(inode);

        let entry = self
            .repository
            .open_entry_by_locator(locator, entry_type)
            .await?;
        Ok(make_file_attr_for_entry(&entry, inode))
    }

    #[allow(clippy::too_many_arguments)]
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
        let mut scope = FormatOptionScope::new(", ");

        log::debug!(
            "setattr {} ({:#o}{}{}{}{:?}{:?}{:?}{}{:?}{:?}{:?}{:#x})",
            self.inodes.path_display(inode, None),
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
        check_unsupported(atime)?;
        check_unsupported(mtime)?;
        check_unsupported(ctime)?;
        check_unsupported(crtime)?;
        check_unsupported(chgtime)?;
        check_unsupported(bkuptime)?;
        check_unsupported(flags)?;

        let mut file = if let Some(handle) = handle {
            MaybeOwnedMut::Borrowed(self.entries.get_file_mut(handle)?)
        } else {
            MaybeOwnedMut::Owned(self.open_file_by_inode(inode).await?)
        };

        if let Some(size) = size {
            file.seek(SeekFrom::Start(size)).await?;
            file.truncate().await?;
        }

        Ok(make_file_attr(inode, EntryType::File, file.len()))
    }

    async fn opendir(&mut self, inode: Inode, flags: OpenFlags) -> Result<FileHandle> {
        log::debug!(
            "opendir {} (flags={})",
            self.inodes.path_display(inode, None),
            flags
        );

        let dir = self.open_directory_by_inode(inode).await?;
        let handle = self.entries.insert(Entry::Directory(dir));

        Ok(handle)
    }

    fn releasedir(&mut self, inode: Inode, handle: FileHandle, flags: OpenFlags) -> Result<()> {
        log::debug!(
            "releasedir {} (handle={}, flags={})",
            self.inodes.path_display(inode, None),
            handle,
            flags
        );

        // TODO: what about `flags`?

        self.entries.get_directory(handle)?;
        self.entries.remove(handle);

        Ok(())
    }

    fn readdir(
        &mut self,
        inode: Inode,
        handle: FileHandle,
        offset: i64,
        reply: &mut ReplyDirectory,
    ) -> Result<()> {
        // Want to keep the `if`s uncollapsed here for readability.
        #![allow(clippy::collapsible_if)]

        log::debug!(
            "readdir {} (handle={}, offset={})",
            self.inodes.path_display(inode, None),
            handle,
            offset
        );

        if offset < 0 {
            return Err(Error::OffsetOutOfRange);
        }

        let parent = self.inodes.get(inode).parent;
        let dir = self.entries.get_directory(handle)?;

        // Handle . and ..
        if offset <= 0 {
            if reply.add(inode, 1, FileType::Directory, ".") {
                return Ok(());
            }
        }

        if offset <= 1 && parent != 0 {
            if reply.add(parent, 2, FileType::Directory, "..") {
                return Ok(());
            }
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
                entry.name(),
            ) {
                break;
            }
        }

        Ok(())
    }

    async fn mkdir(
        &mut self,
        parent: Inode,
        name: &OsStr,
        mode: u32,
        umask: u32,
    ) -> Result<FileAttr> {
        log::debug!(
            "mkdir {} (mode={:#o}, umask={:#o})",
            self.inodes.path_display(parent, Some(name)),
            mode,
            umask
        );

        let mut parent_dir = self.open_directory_by_inode(parent).await?;
        let mut dir = parent_dir.create_subdirectory(name.to_owned())?;

        // TODO: should these two happen atomically (in a transaction)?
        dir.flush().await?;
        parent_dir.flush().await?;

        let inode = self
            .inodes
            .lookup(parent, name, *dir.locator(), EntryType::Directory);

        let entry = Entry::Directory(dir);
        Ok(make_file_attr_for_entry(&entry, inode))
    }

    async fn rmdir(&mut self, parent: Inode, name: &OsStr) -> Result<()> {
        log::debug!("rmdir {}", self.inodes.path_display(parent, Some(name)));

        let mut parent_dir = self.open_directory_by_inode(parent).await?;
        parent_dir.remove_subdirectory(name).await?;
        parent_dir.flush().await
    }

    async fn fsyncdir(&mut self, inode: Inode, handle: FileHandle, datasync: bool) -> Result<()> {
        log::debug!(
            "fsyncdir {} (handle={}, datasync={})",
            self.inodes.path_display(inode, None),
            handle,
            datasync
        );

        // TODO: what about `datasync`?

        let dir = self.entries.get_directory_mut(handle)?;
        dir.flush().await
    }

    async fn create(
        &mut self,
        parent: Inode,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: OpenFlags,
    ) -> Result<(FileAttr, FileHandle, u32)> {
        log::debug!(
            "create {} (mode={:#o}, umask={:#o}, flags={})",
            self.inodes.path_display(parent, Some(name)),
            mode,
            umask,
            flags
        );

        let mut parent_dir = self.open_directory_by_inode(parent).await?;
        let mut file = parent_dir.create_file(name.to_owned())?;

        file.flush().await?;
        parent_dir.flush().await?;

        let entry = Entry::File(file);
        let inode = self
            .inodes
            .lookup(parent, name, *entry.locator(), entry.entry_type());
        let attr = make_file_attr_for_entry(&entry, inode);
        let handle = self.entries.insert(entry);

        Ok((attr, handle, 0))
    }

    async fn open(&mut self, inode: Inode, flags: OpenFlags) -> Result<(FileHandle, u32)> {
        log::debug!(
            "open {} (flags={})",
            self.inodes.path_display(inode, None),
            flags
        );

        // TODO: what about flags (parameter)?

        let file = self.open_file_by_inode(inode).await?;
        let handle = self.entries.insert(Entry::File(file));

        // TODO: what about flags (return value)?

        Ok((handle, 0))
    }

    async fn release(
        &mut self,
        inode: Inode,
        handle: FileHandle,
        flags: OpenFlags,
        flush: bool,
    ) -> Result<()> {
        log::debug!(
            "release {} (handle={}, flags={}, flush={})",
            self.inodes.path_display(inode, None),
            handle,
            flags,
            flush
        );

        // TODO: what about `flags`?

        let file = self.entries.get_file_mut(handle)?;

        if flush {
            file.flush().await?;
        }

        self.entries.remove(handle);

        Ok(())
    }

    async fn read(
        &mut self,
        inode: Inode,
        handle: FileHandle,
        offset: i64,
        size: u32,
        flags: OpenFlags,
    ) -> Result<Vec<u8>> {
        log::debug!(
            "read {} (handle={}, offset={}, size={}, flags={})",
            self.inodes.path_display(inode, None),
            handle,
            offset,
            size,
            flags
        );

        // TODO: what about flags?

        let file = self.entries.get_file_mut(handle)?;

        let offset: u64 = offset.try_into().map_err(|_| Error::OffsetOutOfRange)?;
        file.seek(SeekFrom::Start(offset)).await?;

        // TODO: consider reusing these buffers
        let mut buffer = vec![0; size as usize];
        let len = file.read(&mut buffer).await?;
        buffer.truncate(len);

        Ok(buffer)
    }

    async fn write(
        &mut self,
        inode: Inode,
        handle: FileHandle,
        offset: i64,
        data: &[u8],
        flags: OpenFlags,
    ) -> Result<u32> {
        log::debug!(
            "write {} (handle={}, offset={}, data.len={}, flags={})",
            self.inodes.path_display(inode, None),
            handle,
            offset,
            data.len(),
            flags,
        );

        let offset: u64 = offset.try_into().map_err(|_| Error::OffsetOutOfRange)?;

        let file = self.entries.get_file_mut(handle)?;
        file.seek(SeekFrom::Start(offset)).await?;
        file.write(data).await?;

        Ok(data.len().try_into().unwrap_or(u32::MAX))
    }

    async fn flush(&mut self, inode: Inode, handle: FileHandle) -> Result<()> {
        log::debug!(
            "flush {} (handle={})",
            self.inodes.path_display(inode, None),
            handle
        );
        self.entries.get_file_mut(handle)?.flush().await
    }

    async fn fsync(&mut self, inode: Inode, handle: FileHandle, datasync: bool) -> Result<()> {
        log::debug!(
            "fsync {} (handle={}, datasync={})",
            self.inodes.path_display(inode, None),
            handle,
            datasync
        );

        // TODO: what about `datasync`?
        self.entries.get_file_mut(handle)?.flush().await
    }

    async fn unlink(&mut self, parent: Inode, name: &OsStr) -> Result<()> {
        log::debug!("unlink {}", self.inodes.path_display(parent, Some(name)));

        let mut parent_dir = self.open_directory_by_inode(parent).await?;
        parent_dir.remove_file(name).await?;
        parent_dir.flush().await
    }

    async fn rename(
        &mut self,
        src_parent: Inode,
        src_name: &OsStr,
        dst_parent: Inode,
        dst_name: &OsStr,
        flags: u32,
    ) -> Result<()> {
        log::debug!(
            "rename {} -> {} (flags={:#x})",
            self.inodes.path_display(src_parent, Some(src_name)),
            self.inodes.path_display(dst_parent, Some(dst_name)),
            flags,
        );

        if flags & libc::RENAME_NOREPLACE != 0 {
            return Err(Error::OperationNotSupported);
        }

        if flags & libc::RENAME_EXCHANGE != 0 {
            return Err(Error::OperationNotSupported);
        }

        let mut src_dir = self.open_directory_by_inode(src_parent).await?;
        let mut dst_dir = if src_parent == dst_parent {
            MoveDstDirectory::Src
        } else {
            MoveDstDirectory::Other(self.open_directory_by_inode(dst_parent).await?)
        };

        src_dir.move_entry(src_name, &mut dst_dir, dst_name).await?;
        src_dir.flush().await?;

        if let Some(dir) = dst_dir.get_other() {
            dir.flush().await?;
        }

        Ok(())
    }

    async fn open_file_by_inode(&self, inode: Inode) -> Result<File> {
        let &InodeDetails {
            locator,
            entry_type,
            ..
        } = self.inodes.get(inode);
        entry_type.check_is_file()?;
        self.repository.open_file_by_locator(locator).await
    }

    async fn open_directory_by_inode(&self, inode: Inode) -> Result<Directory> {
        let &InodeDetails {
            locator,
            entry_type,
            ..
        } = self.inodes.get(inode);
        entry_type.check_is_directory()?;
        self.repository.open_directory_by_locator(locator).await
    }
}

fn make_file_attr_for_entry(entry: &Entry, inode: Inode) -> FileAttr {
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
        padding: 0,
        flags: 0,
    }
}

// TODO: consider moving this to `impl Error`
fn to_error_code(error: &Error) -> libc::c_int {
    match error {
        Error::CreateDbDirectory(_)
        | Error::ConnectToDb(_)
        | Error::CreateDbSchema(_)
        | Error::QueryDb(_)
        | Error::BlockNotFound(_)
        | Error::MalformedData
        | Error::MalformedDirectory(_)
        | Error::WrongBlockLength(_)
        | Error::Crypto => libc::EIO,
        Error::EntryNotFound => libc::ENOENT,
        Error::EntryExists => libc::EEXIST,
        Error::EntryNotDirectory => libc::ENOTDIR,
        Error::EntryIsDirectory => libc::EISDIR,
        Error::OffsetOutOfRange => libc::EINVAL,
        Error::DirectoryNotEmpty => libc::ENOTEMPTY,
        Error::OperationNotSupported => libc::ENOSYS,
    }
}

fn to_file_type(entry_type: EntryType) -> FileType {
    match entry_type {
        EntryType::File => FileType::RegularFile,
        EntryType::Directory => FileType::Directory,
    }
}
