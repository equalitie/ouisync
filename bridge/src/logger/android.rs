use super::{common, LogFormat};
use ndk_sys::{
    __android_log_print, android_LogPriority as LogPriority,
    android_LogPriority_ANDROID_LOG_DEBUG as ANDROID_LOG_DEBUG,
    android_LogPriority_ANDROID_LOG_ERROR as ANDROID_LOG_ERROR,
};
use once_cell::sync::Lazy;
use os_pipe::PipeWriter;
use ouisync_tracing_fmt::Formatter;
use paranoid_android::{AndroidLogMakeWriter, Buffer};
use std::{
    ffi::{CStr, CString},
    io::{self, BufRead, BufReader, Stderr, Stdout},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
        raw::c_int,
    },
    path::Path,
    sync::Mutex,
    thread,
};
use tracing_subscriber::{
    fmt::{self, time::SystemTime},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};
// Android log tag.
// HACK: if the tag doesn't start with 'flutter' then the logs won't show up in
// the app if built in release mode.
const TAG: &str = "flutter-ouisync";

pub(super) struct Inner {
    _stdout: Redirect<Stdout, PipeWriter>,
    _stderr: Redirect<Stderr, PipeWriter>,
}

impl Inner {
    pub fn new(path: Option<&Path>, _format: LogFormat) -> io::Result<Self> {
        let android_log_layer = fmt::layer()
            .event_format(Formatter::<()>::default())
            .with_ansi(false)
            .with_writer(AndroidLogMakeWriter::with_buffer(
                TAG.to_owned(),
                Buffer::Main,
            )); // android log adds its own timestamp

        let file_layer = path.map(|path| {
            fmt::layer()
                .event_format(Formatter::<SystemTime>::default())
                .with_ansi(true)
                .with_writer(Mutex::new(common::create_file_writer(path)))
        });

        tracing_subscriber::registry()
            .with(common::create_log_filter())
            .with(android_log_layer)
            .with(file_layer)
            .try_init()
            // `Err` here just means the logger is already initialized, it's OK to ignore it.
            .unwrap_or(());

        Ok(Self {
            _stdout: redirect(io::stdout(), ANDROID_LOG_DEBUG)?,
            _stderr: redirect(io::stderr(), ANDROID_LOG_ERROR)?,
        })
    }
}

fn redirect<S: AsFd>(stream: S, priority: LogPriority) -> io::Result<Redirect<S, PipeWriter>> {
    let (reader, writer) = os_pipe::pipe()?;
    let redirect = Redirect::new(stream, writer)?;

    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            match reader.read_line(&mut line) {
                Ok(n) if n > 0 => {
                    // Remove the trailing newline
                    if line.ends_with('\n') {
                        line.pop();
                    }

                    line = print(priority, line);
                    line.clear();
                }
                Ok(_) => break, // EOF
                Err(error) => {
                    print(ANDROID_LOG_ERROR, error.to_string());
                    break;
                }
            }
        }
    });

    Ok(redirect)
}

// Prints `message` to the android log using zero allocations. Returns the original message.
fn print(priority: LogPriority, message: String) -> String {
    match CString::new(message) {
        Ok(message) => {
            print_cstr(priority, &message);

            // `unwrap` is ok because the `CString` was created from a valid `String`.
            message.into_string().unwrap()
        }
        Err(error) => {
            // message contains internal nul bytes - escape them.

            // `unwrap` is ok because the vector was obtained from a valid `String`.
            let message = String::from_utf8(error.into_vec()).unwrap();
            let escaped = message.replace('\0', "\\0");
            // `unwrap` is ok because we replaced all the internal nul bytes.
            let escaped = CString::new(escaped).unwrap();
            print_cstr(priority, &escaped);

            message
        }
    }
}

fn print_cstr(priority: LogPriority, message: &CStr) {
    static TAG_C: Lazy<CString> = Lazy::new(|| CString::new(TAG).unwrap());

    // SAFETY: both pointers point to valid c-style strings.
    unsafe {
        __android_log_print(priority as c_int, TAG_C.as_ptr(), message.as_ptr());
    }
}

/// Redirect stdout / stderr
struct Redirect<S, D>
where
    S: AsFd,
    D: AsFd,
{
    src: S,
    src_old: OwnedFd,
    _dst: D,
}

impl<S, D> Redirect<S, D>
where
    S: AsFd,
    D: AsFd,
{
    pub fn new(src: S, dst: D) -> io::Result<Self> {
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

impl<S, D> Drop for Redirect<S, D>
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
