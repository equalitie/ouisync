//! Type-safe wrappers around various flags.

use bitflags::bitflags;

bitflags! {
    #[derive(Copy, Clone, Debug)]
    #[repr(transparent)]
    pub(crate) struct OpenFlags : i32 {
        // https://man7.org/linux/man-pages/man2/openat.2.html

        // NOTE: `O_RDONLY` and `O_LARGEFILE` are ignored because they are both 0 so they would
        // otherwise always be printed.
        const WRONLY = libc::O_WRONLY;
        const RDWR = libc::O_RDWR;
        const APPEND = libc::O_APPEND;
        const ASYNC = libc::O_ASYNC;
        const CLOEXEC = libc::O_CLOEXEC;
        const CREAT = libc::O_CREAT;
        const DIRECTORY = libc::O_DIRECTORY;
        const DSYNC = libc::O_DSYNC;
        const EXCL = libc::O_EXCL;
        const NDELAY = libc::O_NDELAY;
        const NOCTTY = libc::O_NOCTTY;
        const NOFOLLOW  = libc::O_NOFOLLOW;
        const NONBLOCK  = libc::O_NONBLOCK;
        const SYNC = libc::O_SYNC;
        const TRUNC = libc::O_TRUNC;
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        const DIRECT = libc::O_DIRECT;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const NOATIME = libc::O_NOATIME;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const PATH  = libc::O_PATH;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const TMPFILE = libc::O_TMPFILE;
    }
}

impl From<i32> for OpenFlags {
    fn from(raw: i32) -> Self {
        Self::from_bits_retain(raw)
    }
}

bitflags! {
    #[derive(Copy, Clone, Debug)]
    #[repr(transparent)]
    pub(crate) struct RenameFlags : u32 {
        const NOREPLACE = libc::RENAME_NOREPLACE;
        const EXCHANGE = libc::RENAME_EXCHANGE;
        const WHITEOUT = libc::RENAME_WHITEOUT;
    }
}

impl From<u32> for RenameFlags {
    fn from(raw: u32) -> Self {
        Self::from_bits_retain(raw)
    }
}
