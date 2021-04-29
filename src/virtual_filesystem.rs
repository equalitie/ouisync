use crate::inode::{Inode, InodeMap};
use fuser::{
    BackgroundSession, FileAttr, FileType, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    ReplyWrite, Request,
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

type FileHandle = u64;

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
    repository: Repository,
    inodes: InodeMap,
}

impl VirtualFilesystem {
    fn new(runtime_handle: tokio::runtime::Handle, repository: Repository) -> Self {
        Self {
            rt: runtime_handle,
            repository,
            inodes: InodeMap::new(),
        }
    }
}

impl fuser::Filesystem for VirtualFilesystem {
    fn lookup(&mut self, _req: &Request, parent: Inode, name: &OsStr, reply: ReplyEntry) {
        log::debug!("lookup (parent={}, name={:?})", parent, name);

        let (locator, entry_type) = try_request!(self.inodes.get(parent), reply);

        let repository = &self.repository;
        let inodes = &mut self.inodes;

        self.rt.block_on(async {
            let dir = match try_request!(repository.open_entry(locator, entry_type).await, reply) {
                Entry::Directory(dir) => dir,
                _ => {
                    reply.error(libc::ENOTDIR);
                    return;
                }
            };

            let entry_info = try_request!(dir.lookup(name), reply);
            let entry = try_request!(entry_info.open().await, reply);

            let inode = inodes.lookup(
                parent,
                name.to_owned(),
                entry_info.locator(),
                entry_info.entry_type(),
            );

            let attr = get_file_attr(&entry, inode);
            reply.entry(&TTL, &attr, 0)
        })
    }

    fn forget(&mut self, _req: &Request, inode: Inode, lookups: u64) {
        log::debug!("forget (inode={}, lookups={})", inode, lookups);
        self.inodes.forget(inode, lookups)
    }

    fn getattr(&mut self, _req: &Request, inode: Inode, reply: ReplyAttr) {
        log::debug!("getattr (inode={})", inode);

        let (locator, entry_type) = try_request!(self.inodes.get(inode), reply);

        self.rt.block_on(async {
            let entry = try_request!(self.repository.open_entry(locator, entry_type).await, reply);
            let attr = get_file_attr(&entry, inode);
            reply.attr(&TTL, &attr)
        })
    }

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

    fn readdir(
        &mut self,
        _req: &Request,
        _inode: Inode,
        _handle: FileHandle,
        _offset: i64,
        _reply: ReplyDirectory,
    ) {
        // log::debug!("readdir ino={}, offset={}", ino, offset);
        // let entries = match self.entries.get(&ino) {
        //     Some(Entry::Directory(entries)) => entries,
        //     Some(Entry::File(_)) => {
        //         reply.error(libc::ENOTDIR);
        //         return;
        //     }
        //     None => {
        //         reply.error(libc::ENOENT);
        //         return;
        //     }
        // };

        // // TODO: . and ..

        // for (index, (name, &inode)) in entries.iter().enumerate().skip(offset as usize) {
        //     let file_type = match self.entries.get(&inode) {
        //         Some(Entry::File(_)) => FileType::RegularFile,
        //         Some(Entry::Directory(_)) => FileType::Directory,
        //         None => {
        //             reply.error(libc::ENOENT);
        //             return;
        //         }
        //     };

        //     if reply.add(inode, (index + 3) as i64, file_type, name) {
        //         break;
        //     }
        // }

        // reply.ok()

        todo!()
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        _parent: Inode,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        _reply: ReplyEntry,
    ) {
        todo!()

        // log::debug!("mkdir parent={}, name={:?}", parent, name);
        // self.make_entry(parent, name, Entry::Directory(HashMap::new()), reply)
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

fn get_file_attr(entry: &Entry, inode: Inode) -> FileAttr {
    let kind = match entry.entry_type() {
        EntryType::File => FileType::RegularFile,
        EntryType::Directory => FileType::Directory,
    };

    FileAttr {
        ino: inode,
        size: entry.len(),
        blocks: 0,                      // TODO: ?
        atime: SystemTime::UNIX_EPOCH,  // TODO
        mtime: SystemTime::UNIX_EPOCH,  // TODO
        ctime: SystemTime::UNIX_EPOCH,  // TODO
        crtime: SystemTime::UNIX_EPOCH, // TODO
        kind,
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
        | Error::WrongBlockLength(_)
        | Error::Crypto => libc::EIO,
        Error::BlockIdNotFound | Error::BlockNotFound(_) | Error::EntryNotFound => libc::ENOENT,
        Error::MalformedDirectory(_) => libc::ENOTDIR,
        Error::EntryExists => libc::EEXIST,
    }
}
