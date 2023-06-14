//! stdout and stderr redirection

use std::{io, os::fd::AsRawFd};

pub(crate) struct Redirect<S, D>
where
    S: AsRawFd,
    D: AsRawFd,
{
    src: S,
    src_old_fd: libc::c_int,
    _dst: D,
}

impl<S, D> Redirect<S, D>
where
    S: AsRawFd,
    D: AsRawFd,
{
    pub fn new(src: S, dst: D) -> io::Result<Self> {
        // Remember the old fd so we can point it to where it pointed before when we are done.
        //
        // SAFETY: The file descriptor should be valid because it was obtained using
        // `as_raw_fd` from a valid rust io object.
        let src_old_fd = unsafe { libc::dup(src.as_raw_fd()) };

        // SAFETY: Both file descriptors should be valid because they are obtained using
        // `as_raw_fd` from valid rust io objects.
        unsafe {
            if libc::dup2(dst.as_raw_fd(), src.as_raw_fd()) < 0 {
                return Err(io::Error::last_os_error());
            }
        }

        Ok(Self {
            src,
            src_old_fd,
            _dst: dst,
        })
    }
}

impl<S, D> Drop for Redirect<S, D>
where
    S: AsRawFd,
    D: AsRawFd,
{
    fn drop(&mut self) {
        unsafe {
            // Point the original FD to it's previous target.
            let status = libc::dup2(self.src_old_fd, self.src.as_raw_fd());

            if status < 0 {
                tracing::error!(
                    "Failed to point the redirected file descriptor to its original target \
                     (error code: {status})",
                );
            }
        }
    }
}
