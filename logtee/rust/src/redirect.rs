// Implementation based on redirecting stdout to the log file.

use std::{
    io, panic,
    thread::{self, JoinHandle},
};

use file_rotate::{FileRotate, suffix::AppendCount};
use implementation::{Redirect, clone_stdout};

pub(super) struct Inner {
    redirect: Redirect,
    handle: JoinHandle<io::Result<()>>,
}

impl Inner {
    pub fn new(file: FileRotate<AppendCount>) -> io::Result<Self> {
        let mut file = strip_ansi_escapes::Writer::new(file);
        let mut orig_stdout = clone_stdout()?;
        let (mut pipe_reader, pipe_writer) = io::pipe()?;

        let redirect = Redirect::start(pipe_writer)?;

        // Read from the pipe reader and write to the original stdout and the file.
        let handle = thread::spawn(move || tee(&mut pipe_reader, &mut orig_stdout, &mut file));

        Ok(Self { redirect, handle })
    }

    pub fn close(self) -> io::Result<()> {
        self.redirect.stop().ok();

        match self.handle.join() {
            Ok(result) => result,
            Err(payload) => panic::resume_unwind(payload),
        }
    }
}

// Read from `src` and write to both `dst0` and `dst1`.
pub(crate) fn tee(
    src: &mut impl io::Read,
    dst0: &mut impl io::Write,
    dst1: &mut impl io::Write,
) -> io::Result<()> {
    let mut buffer = vec![0; 1024];

    loop {
        let n = src.read(&mut buffer)?;

        if n > 0 {
            dst0.write_all(&buffer[..n])?;
            dst1.write_all(&buffer[..n])?;
        } else {
            return Ok(());
        }
    }
}

#[cfg(target_os = "linux")]
mod implementation {
    use std::{
        fs::File,
        io::{self, PipeWriter},
        os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
    };

    pub(super) struct Redirect {
        orig: OwnedFd,
    }

    impl Redirect {
        pub fn start(pipe_writer: PipeWriter) -> io::Result<Self> {
            let orig = io::stdout().as_fd().try_clone_to_owned()?;

            dup2(pipe_writer.as_fd(), io::stdout().as_fd())?;
            drop(pipe_writer);

            Ok(Self { orig })
        }

        pub fn stop(self) -> io::Result<()> {
            dup2(self.orig.as_fd(), io::stdout().as_fd())
        }
    }

    pub(super) fn clone_stdout() -> io::Result<impl io::Write> {
        Ok(File::from(io::stdout().as_fd().try_clone_to_owned()?))
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
}

#[cfg(target_os = "windows")]
mod implementation {
    use std::{
        fs::File,
        io::{self, PipeWriter},
        os::windows::io::{AsHandle, AsRawHandle, RawHandle},
    };

    use winapi::um::{
        handleapi::INVALID_HANDLE_VALUE,
        processenv::{GetStdHandle, SetStdHandle},
        winbase::STD_OUTPUT_HANDLE,
    };

    pub(super) struct Redirect {
        orig_stdout: RawHandle,
        #[expect(dead_code)]
        pipe_writer: PipeWriter,
    }

    impl Redirect {
        pub fn start(pipe_writer: PipeWriter) -> io::Result<Self> {
            let orig_stdout = get_stdout()?;

            // SAFETY: the argument is a valid handle obtained from the pipe writer.
            unsafe {
                set_stdout(pipe_writer.as_raw_handle())?;
            }

            Ok(Self {
                orig_stdout,
                pipe_writer,
            })
        }

        pub fn stop(self) -> io::Result<()> {
            // SAFETY: the argument is a valid handle (or `null`[^1]) obtained by calling
            // `get_stdout`.
            //
            // [^1]: The documentation
            // (https://learn.microsoft.com/en-us/windows/console/setstdhandle) doesn't say
            // anything about passing `null` handles so I'm assuming it is safe.
            unsafe { set_stdout(self.orig_stdout) }
        }
    }

    pub(super) fn clone_stdout() -> io::Result<impl io::Write> {
        Ok(File::from(io::stdout().as_handle().try_clone_to_owned()?))
    }

    // SAFETY: `dst` must be a valid handle
    unsafe fn set_stdout(dst: RawHandle) -> io::Result<()> {
        let result = unsafe { SetStdHandle(STD_OUTPUT_HANDLE, dst as _) };

        if result != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn get_stdout() -> io::Result<RawHandle> {
        // SAFETY: There doesn't seem to be anything potentially unsound here.
        let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };

        if handle != INVALID_HANDLE_VALUE {
            Ok(handle as _)
        } else {
            Err(io::Error::last_os_error())
        }
    }
}
