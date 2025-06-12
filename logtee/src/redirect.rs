// Implementation based on redirecting stdout to the log file.

use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
    panic,
    path::Path,
    thread::{self, JoinHandle},
};

use super::{create_file, RotateOptions};

pub(super) struct Inner {
    orig: OwnedFd,
    handle: JoinHandle<io::Result<()>>,
}

impl Inner {
    pub fn new(path: &Path, options: RotateOptions) -> io::Result<Self> {
        let mut file = strip_ansi_escapes::Writer::new(create_file(path, options));

        // Create anonymous pipe and redirect stdout to the write end of it. Also keep the
        // original (not redirected) stdout around for later.
        let (mut pipe_reader, pipe_writer) = io::pipe()?;
        let orig = io::stdout().as_fd().try_clone_to_owned()?;

        dup2(pipe_writer.as_fd(), io::stdout().as_fd())?;
        drop(pipe_writer);

        // Read from the pipe reader and write to the original stdout and the file.
        let handle = thread::spawn({
            let mut orig = File::from(orig.try_clone()?);

            move || {
                let mut buffer = vec![0; 1024];

                loop {
                    // When the pipe writer drops, this returns 0 (signalling EOF) and this
                    // thread terminates.
                    let n = pipe_reader.read(&mut buffer)?;
                    if n > 0 {
                        orig.write_all(&buffer[..n])?;
                        file.write_all(&buffer[..n])?;
                    } else {
                        return Ok(());
                    }
                }
            }
        });

        Ok(Self { orig, handle })
    }

    pub fn close(self) -> io::Result<()> {
        // Restore the original stdout. This releases the pipe writer which causes the pipe
        // reader to reach EOF and the thread to terminate.
        dup2(self.orig.as_fd(), io::stdout().as_fd()).ok();
        drop(self.orig);

        match self.handle.join() {
            Ok(result) => result,
            Err(payload) => panic::resume_unwind(payload),
        }
    }
}

// Safe wrappper over libc::dup2
fn dup2(dst: BorrowedFd<'_>, src: BorrowedFd<'_>) -> io::Result<()> {
    // SAFETY: Both file descriptors are valid because they are obtained using `as_raw_fd`
    // from valid io objects.
    unsafe {
        if libc::dup2(dst.as_raw_fd(), src.as_raw_fd()) >= 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}
