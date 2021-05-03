mod entry_map;
mod handle_generator;
mod inode;

use self::{
    entry_map::{EntryMap, FileHandle},
    inode::{Inode, InodeDetails, InodeMap},
};
use fuser::{
    BackgroundSession, FileAttr, FileType, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, ReplyWrite, Request,
};
use ouisync::{Entry, EntryType, Error, Repository, Result};
use std::{
    ffi::OsStr,
    io,
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
                inodes: InodeMap::default(),
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

    fn forget(&mut self, _req: &Request, inode: Inode, lookups: u64) {
        log::debug!("forget (inode={}, lookups={})", inode, lookups);
        self.inner.inodes.forget(inode, lookups)
    }

    fn getattr(&mut self, _req: &Request, inode: Inode, reply: ReplyAttr) {
        let attr = try_request!(self.rt.block_on(self.inner.getattr(inode)), reply);
        reply.attr(&TTL, &attr)
    }

    fn opendir(&mut self, _req: &Request, inode: Inode, flags: i32, reply: ReplyOpen) {
        let handle = try_request!(self.rt.block_on(self.inner.opendir(inode, flags)), reply);
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
        log::debug!(
            "releasedir (inode={}, handle={}, flags={:#x})",
            inode,
            handle,
            flags
        );

        // TODO: `forget` the inodes looked up during `readdir`
        // TODO: what about `flags`?

        let _ = self.inner.entries.remove(handle);
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

    // fn fsyncdir(
    //     &mut self,
    //     _req: &Request<'_>,
    //     inode: u64,
    //     handle: u64,
    //     _datasync: bool,
    //     reply: ReplyEmpty,
    // ) {
    //     log::debug!("fsyncdir (inode = {}, handle = {})", inode, handle);
    //     reply.error(libc::ENOSYS);
    // }

    fn read(
        &mut self,
        _req: &Request,
        _inode: Inode,
        _handle: FileHandle,
        _offset: i64,
        _size: u32,
        _flags: i32,
        _lock: Option<u64>,
        _reply: ReplyData,
    ) {
        todo!()

        // log::debug!("read ino={}, offset={}, size={}", ino, offset, size);

        // let content = match self.entries.get(&ino) {
        //     Some(Entry::File(content)) => content,
        //     Some(Entry::Directory(_)) => {
        //         reply.error(libc::EISDIR);
        //         return;
        //     }
        //     None => {
        //         reply.error(libc::ENOENT);
        //         return;
        //     }
        // };

        // let start = (offset as usize).min(content.len());
        // let end = (start + size as usize).min(content.len());

        // reply.data(&content[start..end]);
    }

    fn write(
        &mut self,
        _req: &Request<'_>,
        _inode: Inode,
        _handle: FileHandle,
        _offset: i64,
        _data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        _reply: ReplyWrite,
    ) {
        todo!()

        // log::debug!("write ino={}, offset={}, size={}", ino, offset, data.len());

        // let content = match self.entries.get_mut(&ino) {
        //     Some(Entry::File(content)) => content,
        //     Some(Entry::Directory(_)) => {
        //         reply.error(libc::EISDIR);
        //         return;
        //     }
        //     None => {
        //         reply.error(libc::ENOENT);
        //         return;
        //     }
        // };

        // let offset = offset as usize; // FIMXE: use `usize::try_from` instead
        // let new_len = content.len().max(offset + data.len());

        // if new_len > content.len() {
        //     content.resize(new_len, 0)
        // }

        // content[offset..offset + data.len()].copy_from_slice(data);

        // reply.written(data.len() as u32)
    }

    fn mknod(
        &mut self,
        _req: &Request<'_>,
        _parent: Inode,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        _rdev: u32,
        _reply: ReplyEntry,
    ) {
        todo!()

        // log::debug!("mknod parent={}, name={:?}", parent, name);
        // self.make_entry(parent, name, Entry::File(vec![]), reply)
    }
}

struct Inner {
    repository: Repository,
    inodes: InodeMap,
    entries: EntryMap,
}

impl Inner {
    async fn lookup(&mut self, parent: Inode, name: &OsStr) -> Result<FileAttr> {
        log::debug!("lookup (parent={}, name={:?})", parent, name);

        let InodeDetails {
            locator,
            entry_type,
            ..
        } = *self.inodes.get(parent)?;
        check_is_directory(entry_type)?;

        let parent_dir = self.repository.open_directory(locator).await?;
        let entry_info = parent_dir.lookup(name)?;
        let entry = entry_info.open().await?;

        let inode = self.inodes.lookup(
            parent,
            name.to_owned(),
            entry_info.locator(),
            entry_info.entry_type(),
        );

        Ok(get_file_attr(&entry, inode))
    }

    async fn getattr(&mut self, inode: Inode) -> Result<FileAttr> {
        log::debug!("getattr (inode={})", inode);

        let InodeDetails {
            locator,
            entry_type,
            ..
        } = *self.inodes.get(inode)?;

        let entry = self.repository.open_entry(locator, entry_type).await?;
        Ok(get_file_attr(&entry, inode))
    }

    async fn opendir(&mut self, inode: Inode, flags: i32) -> Result<FileHandle> {
        log::debug!("opendir (inode={}, flags={:#x})", inode, flags);

        let InodeDetails {
            locator,
            entry_type,
            ..
        } = *self.inodes.get(inode)?;
        check_is_directory(entry_type)?;

        let dir = self.repository.open_directory(locator).await?;
        let handle = self.entries.insert(Entry::Directory(dir));

        Ok(handle)
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
            "readdir (inode={}, handle={}, offset={})",
            inode,
            handle,
            offset
        );

        // TODO:
        // if offset < 0 {
        //     log::error!("negative offset not allowed");
        //     reply.error(libc::EINVAL);
        //     return;
        // }

        let parent = self.inodes.get(inode)?.parent;
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
            // FIXME: according to https://libfuse.github.io/doxygen/structfuse__lowlevel__ops.html#af1ef8e59e0cb0b02dc0e406898aeaa51:
            // > Returning a directory entry from readdir() does not affect its lookup count.
            let entry_inode = self.inodes.lookup(
                inode,
                entry.name().to_owned(),
                entry.locator(),
                entry.entry_type(),
            );

            if reply.add(
                entry_inode,
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
            "mkdir (parent={}, name={:?}, mode={:#o}, umask={:#o})",
            parent,
            name,
            mode,
            umask
        );

        let InodeDetails {
            locator,
            entry_type,
            ..
        } = *self.inodes.get(parent)?;
        check_is_directory(entry_type)?;

        let mut parent_dir = self.repository.open_directory(locator).await?;
        let mut dir = parent_dir.create_subdirectory(name.to_owned())?;

        // TODO: should these two happen atomically (in a transaction)?
        dir.flush().await?;
        parent_dir.flush().await?;

        // TODO: when do we `forget` this lookup?
        let inode = self.inodes.lookup(
            parent,
            name.to_owned(),
            *dir.locator(),
            EntryType::Directory,
        );

        let entry = Entry::Directory(dir);
        Ok(get_file_attr(&entry, inode))
    }
}

fn get_file_attr(entry: &Entry, inode: Inode) -> FileAttr {
    FileAttr {
        ino: inode,
        size: entry.len(),
        blocks: 0,                      // TODO: ?
        atime: SystemTime::UNIX_EPOCH,  // TODO
        mtime: SystemTime::UNIX_EPOCH,  // TODO
        ctime: SystemTime::UNIX_EPOCH,  // TODO
        crtime: SystemTime::UNIX_EPOCH, // TODO
        kind: to_file_type(entry.entry_type()),
        perm: match entry.entry_type() {
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
        | Error::MalformedData
        | Error::MalformedDirectory(_)
        | Error::WrongBlockLength(_)
        | Error::Crypto => libc::EIO,
        Error::BlockIdNotFound | Error::BlockNotFound(_) | Error::EntryNotFound => libc::ENOENT,
        Error::EntryExists => libc::EEXIST,
        Error::EntryNotDirectory => libc::ENOTDIR,
    }
}

fn to_file_type(entry_type: EntryType) -> FileType {
    match entry_type {
        EntryType::File => FileType::RegularFile,
        EntryType::Directory => FileType::Directory,
    }
}

fn check_is_directory(entry_type: EntryType) -> Result<()> {
    match entry_type {
        EntryType::Directory => Ok(()),
        _ => Err(Error::EntryNotDirectory),
    }
}
