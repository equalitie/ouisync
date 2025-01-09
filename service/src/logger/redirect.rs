use std::{
    io::{self, BufRead, BufReader, Stderr, Stdout},
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
    thread,
};

use os_pipe::PipeWriter;
use tracing::{Level, Span};

/// Redirect stdout / stderr to tracing events
pub(super) struct Redirect {
    _stdout: Guard<Stdout, PipeWriter>,
    _stderr: Guard<Stderr, PipeWriter>,
}

impl Redirect {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            _stdout: redirect(io::stdout(), tracing::info_span!("stdout"), Level::INFO)?,
            _stderr: redirect(io::stderr(), tracing::info_span!("stderr"), Level::ERROR)?,
        })
    }
}

fn redirect<S: AsFd>(stream: S, span: Span, level: Level) -> io::Result<Guard<S, PipeWriter>> {
    let (reader, writer) = os_pipe::pipe()?;
    let redirect = Guard::new(stream, writer)?;

    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        let _enter = span.enter();

        loop {
            match reader.read_line(&mut line) {
                Ok(n) if n > 0 => {
                    // Remove the trailing newline
                    if line.ends_with('\n') {
                        line.pop();
                    }

                    // Unfornutally it doesn't seem to be possible to pass non constant level to
                    // `tracing::event!`...
                    match level {
                        Level::ERROR => tracing::error!("{line}"),
                        Level::WARN => tracing::warn!("{line}"),
                        Level::INFO => tracing::info!("{line}"),
                        Level::DEBUG => tracing::debug!("{line}"),
                        Level::TRACE => tracing::trace!("{line}"),
                    }
                }
                Ok(_) => break, // EOF
                Err(error) => {
                    tracing::error!("{error:?}");
                    break;
                }
            }
        }
    });

    Ok(redirect)
}

/// Redirect stdout / stderr
struct Guard<S, D>
where
    S: AsFd,
    D: AsFd,
{
    src: S,
    src_old: OwnedFd,
    _dst: D,
}

impl<S, D> Guard<S, D>
where
    S: AsFd,
    D: AsFd,
{
    fn new(src: S, dst: D) -> io::Result<Self> {
        // Remember the old fd so we can point it to where it pointed before when we are done.
        let src_old = src.as_fd().try_clone_to_owned()?;

        dup2(dst.as_fd(), src.as_fd())?;

        Ok(Self {
            src,
            src_old,
            _dst: dst,
        })
    }
}

impl<S, D> Drop for Guard<S, D>
where
    S: AsFd,
    D: AsFd,
{
    fn drop(&mut self) {
        if let Err(error) = dup2(self.src_old.as_fd(), self.src.as_fd()) {
            tracing::error!(
                ?error,
                "Failed to point the redirected file descriptor to its original target"
            );
        }
    }
}

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
