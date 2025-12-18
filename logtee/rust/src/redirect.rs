// Implementation based on redirecting stdout to the log file.

use std::{
    io::{self, PipeWriter},
    panic,
    thread::{self, JoinHandle},
};

use file_rotate::{FileRotate, suffix::AppendCount};
use filedescriptor::{Error, FileDescriptor, StdioDescriptor};

pub(super) struct Inner {
    orig_stdout: FileDescriptor,
    pipe_writer: PipeWriter,
    handle: JoinHandle<io::Result<()>>,

    #[cfg(windows)]
    dummy_console: bool,
}

impl Inner {
    pub fn new(file: FileRotate<AppendCount>) -> io::Result<Self> {
        let mut file = strip_ansi_escapes::Writer::new(file);
        let (mut pipe_reader, pipe_writer) = io::pipe()?;

        #[cfg(windows)]
        let dummy_console = create_console();

        let orig_stdout = FileDescriptor::redirect_stdio(&pipe_writer, StdioDescriptor::Stdout)
            .map_err(into_io_error)?;

        // Read from the pipe reader and write to the original stdout and the file.
        let handle = thread::spawn({
            let mut orig_stdout = orig_stdout.as_file().map_err(into_io_error)?;
            move || tee(&mut pipe_reader, &mut orig_stdout, &mut file)
        });

        Ok(Self {
            orig_stdout,
            pipe_writer,
            handle,
            #[cfg(windows)]
            dummy_console,
        })
    }

    pub fn close(self) -> io::Result<()> {
        let Self {
            orig_stdout,
            pipe_writer,
            handle,
            #[cfg(windows)]
            dummy_console,
        } = self;

        FileDescriptor::redirect_stdio(&orig_stdout, StdioDescriptor::Stdout)
            .map_err(into_io_error)?;

        drop(pipe_writer);

        #[cfg(windows)]
        if dummy_console {
            destroy_console();
        }

        match handle.join() {
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

fn into_io_error(error: Error) -> io::Error {
    match error {
        Error::Pipe(error)
        | Error::Socketpair(error)
        | Error::Bind(error)
        | Error::Getsockname(error)
        | Error::Listen(error)
        | Error::Connect(error)
        | Error::Accept(error)
        | Error::Fcntl(error)
        | Error::Cloexec(error)
        | Error::FionBio(error)
        | Error::Poll(error)
        | Error::Dup { source: error, .. }
        | Error::Dup2 { source: error, .. }
        | Error::SetStdHandle(error)
        | Error::Io(error) => error,
        _ => io::Error::other(error),
    }
}

// HACK: If the app doesn't have a console attached, it doesn't seem to be possible to redirect
// stdout. So as a workaround, we create a dummy console and immediately hide its window. If the
// app already has a console, we do nothing.
//
// Idea taken from https://stackoverflow.com/a/51825980/170073
#[cfg(windows)]
fn create_console() -> bool {
    use std::{ffi::CStr, ptr};
    use winapi::um::{
        consoleapi::AllocConsole,
        processenv::GetStdHandle,
        winbase::STD_OUTPUT_HANDLE,
        winuser::{FindWindowA, ShowWindow},
    };

    unsafe {
        if GetStdHandle(STD_OUTPUT_HANDLE) != ptr::null_mut() {
            // stdout exists, not need to create the dummy console
            return false;
        }

        if AllocConsole() == 0 {
            return false;
        }

        ShowWindow(
            FindWindowA(
                CStr::from_bytes_with_nul(b"ConsoleWindowClass\0")
                    .unwrap()
                    .as_ptr(),
                ptr::null(),
            ),
            0,
        );

        true
    }
}

#[cfg(windows)]
fn destroy_console() {
    use winapi::um::wincon::FreeConsole;

    unsafe {
        FreeConsole();
    }
}
