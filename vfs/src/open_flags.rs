use std::fmt;

// Type-safe wrapper around the file open flags.
pub struct OpenFlags(i32);

impl OpenFlags {
    pub fn contains(&self, bit: i32) -> bool {
        if bit == 0 {
            self.0 & libc::O_ACCMODE == 0
        } else {
            self.0 & bit == bit
        }
    }
}

impl From<i32> for OpenFlags {
    fn from(raw: i32) -> Self {
        Self(raw)
    }
}

impl From<OpenFlags> for i32 {
    fn from(flags: OpenFlags) -> Self {
        flags.0
    }
}

impl fmt::Display for OpenFlags {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut bar = false;
        let mut write = |bit: i32, name: &'static str| {
            if self.contains(bit) {
                if bar {
                    write!(f, "|")?;
                }

                write!(f, "{}", name)?;
                bar = true;
            }

            Ok(())
        };

        // https://man7.org/linux/man-pages/man2/openat.2.html

        // NOTE: `O_RDONLY` and `O_LARGEFILE` are ignored because they are both 0 so they would
        // otherwise always be printed.

        write(libc::O_WRONLY, "WRONLY")?;
        write(libc::O_RDWR, "RDWR")?;
        write(libc::O_APPEND, "APPEND")?;
        write(libc::O_ASYNC, "ASYNC")?;
        write(libc::O_CLOEXEC, "CLOEXEC")?;
        write(libc::O_CREAT, "CREAT")?;
        write(libc::O_DIRECTORY, "DIRECTORY")?;
        write(libc::O_DSYNC, "DSYNC")?;
        write(libc::O_EXCL, "EXCL")?;
        write(libc::O_NOCTTY, "NOCTTY")?;
        write(libc::O_NOFOLLOW, "NOFOLLOW")?;
        write(libc::O_NONBLOCK, "NONBLOCK")?;
        write(libc::O_SYNC, "SYNC")?;
        write(libc::O_TRUNC, "TRUNC")?;

        #[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd", target_os = "netbsd", target_os = "dragonfly"))]
        write(libc::O_DIRECT, "DIRECT")?;

        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            write(libc::O_NOATIME, "NOATIME")?;
            write(libc::O_PATH, "PATH")?;
            write(libc::O_TMPFILE, "TMPFILE")?;
        }

        Ok(())
    }
}
