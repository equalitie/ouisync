use fuser::{
    BackgroundSession, FileAttr, FileType, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    ReplyWrite, Request,
};
use ouisync::Repository;
use std::{
    array,
    collections::HashMap,
    ffi::{OsStr, OsString},
    io,
    path::Path,
    time::{Duration, UNIX_EPOCH},
};
use tokio::task;

// NOTE: this is the unix implementation of virtual filesystem which is backed by fuse. Eventually
// there will be one for windows as well with the same public API but backed (probably) by
// [dokan](https://github.com/dokan-dev/dokan-rust).

/// Mount `repository` under the given directory. Spawns the filesystem handler in a background
/// thread and immediatelly returns. The returned `MountGuard` unmouts the repository on drop.
pub fn mount(
    repository: Repository,
    mount_point: impl AsRef<Path>,
) -> Result<MountGuard, io::Error> {
    let session = fuser::spawn_mount(VirtualFilesystem::new(repository), mount_point, &[])?;
    Ok(MountGuard(session))
}

/// Unmounts the virtual filesystem when dropped.
pub struct MountGuard(BackgroundSession);

struct VirtualFilesystem {
    entries: HashMap<u64, Entry>,
}

impl VirtualFilesystem {
    fn new(_repository: Repository) -> Self {
        let mut root_content = HashMap::new();
        let _ = root_content.insert("foo.txt".into(), 2);
        let _ = root_content.insert("bar.txt".into(), 3);

        let mut entries = HashMap::new();
        let _ = entries.insert(1, Entry::Directory(root_content));
        let _ = entries.insert(2, Entry::File(b"content of foo".to_vec()));
        let _ = entries.insert(3, Entry::File(b"content of bar".to_vec()));

        Self { entries }
    }

    fn new_inode(&self) -> u64 {
        self.entries.keys().max().copied().unwrap_or(0) + 1
    }

    fn make_entry(&mut self, parent: u64, name: &OsStr, new_entry: Entry, reply: ReplyEntry) {
        let new_inode = self.new_inode();

        let parent_entries = match self.entries.get_mut(&parent) {
            Some(Entry::Directory(entries)) => entries,
            Some(Entry::File(_)) => {
                reply.error(libc::ENOTDIR);
                return;
            }
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        if parent_entries.contains_key(name) {
            reply.error(libc::EEXIST);
            return;
        }

        let attr = new_entry.attr(new_inode);

        let _ = parent_entries.insert(name.to_owned(), new_inode);
        let _ = self.entries.insert(new_inode, new_entry);

        reply.entry(&Duration::default(), &attr, 0)
    }
}

impl fuser::Filesystem for VirtualFilesystem {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        log::debug!("lookup parent={}, name={:?}", parent, name);

        let entry = if let Some(entry) = self.entries.get(&parent) {
            entry
        } else {
            reply.error(libc::ENOENT);
            return;
        };

        let inode = match entry {
            Entry::File(_) => {
                reply.error(libc::ENOTDIR);
                return;
            }
            Entry::Directory(entries) => {
                if let Some(&inode) = entries.get(name) {
                    inode
                } else {
                    reply.error(libc::ENOENT);
                    return;
                }
            }
        };

        let attr = match self.entries.get(&inode) {
            Some(entry) => entry.attr(inode),
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        reply.entry(&Duration::default(), &attr, 0)
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        log::debug!("getattr ino={}", ino);

        if let Some(entry) = self.entries.get(&ino) {
            reply.attr(&Duration::default(), &entry.attr(ino))
        } else {
            reply.error(libc::ENOENT)
        }
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        log::debug!("read ino={}, offset={}, size={}", ino, offset, size);

        let content = match self.entries.get(&ino) {
            Some(Entry::File(content)) => content,
            Some(Entry::Directory(_)) => {
                reply.error(libc::EISDIR);
                return;
            }
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let start = (offset as usize).min(content.len());
        let end = (start + size as usize).min(content.len());

        reply.data(&content[start..end]);
    }

    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        log::debug!("write ino={}, offset={}, size={}", ino, offset, data.len());

        let content = match self.entries.get_mut(&ino) {
            Some(Entry::File(content)) => content,
            Some(Entry::Directory(_)) => {
                reply.error(libc::EISDIR);
                return;
            }
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let offset = offset as usize; // FIMXE: use `usize::try_from` instead
        let new_len = content.len().max(offset + data.len());

        if new_len > content.len() {
            content.resize(new_len, 0)
        }

        content[offset..offset + data.len()].copy_from_slice(data);

        reply.written(data.len() as u32)
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        log::debug!("readdir ino={}, offset={}", ino, offset);

        let entries = match self.entries.get(&ino) {
            Some(Entry::Directory(entries)) => entries,
            Some(Entry::File(_)) => {
                reply.error(libc::ENOTDIR);
                return;
            }
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        // TODO: . and ..

        for (index, (name, &inode)) in entries.iter().enumerate().skip(offset as usize) {
            let file_type = match self.entries.get(&inode) {
                Some(Entry::File(_)) => FileType::RegularFile,
                Some(Entry::Directory(_)) => FileType::Directory,
                None => {
                    reply.error(libc::ENOENT);
                    return;
                }
            };

            if reply.add(inode, (index + 3) as i64, file_type, name) {
                break;
            }
        }

        reply.ok()
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        log::debug!("mkdir parent={}, name={:?}", parent, name);
        self.make_entry(parent, name, Entry::Directory(HashMap::new()), reply)
    }

    fn mknod(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        log::debug!("mknod parent={}, name={:?}", parent, name);
        self.make_entry(parent, name, Entry::File(vec![]), reply)
    }
}

enum Entry {
    File(Vec<u8>),
    Directory(HashMap<OsString, u64>),
}

impl Entry {
    fn attr(&self, inode: u64) -> FileAttr {
        match self {
            Self::File(content) => FileAttr {
                ino: inode,
                size: content.len() as u64,
                blocks: 0,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: FileType::RegularFile,
                perm: 0o444,
                nlink: 1,
                uid: 0,
                gid: 0,
                rdev: 0,
                blksize: 0,
                padding: 0,
                flags: 0,
            },
            Self::Directory(_) => FileAttr {
                ino: inode,
                size: 0,
                blocks: 0,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: FileType::Directory,
                perm: 0o555,
                nlink: 1,
                uid: 0,
                gid: 0,
                rdev: 0,
                blksize: 0,
                padding: 0,
                flags: 0,
            },
        }
    }
}
