use ndk_sys::{
    __android_log_print, android_LogPriority as LogPriority,
    android_LogPriority_ANDROID_LOG_DEBUG as ANDROID_LOG_DEBUG,
    android_LogPriority_ANDROID_LOG_ERROR as ANDROID_LOG_ERROR,
    android_LogPriority_ANDROID_LOG_FATAL as ANDROID_LOG_FATAL,
};
use once_cell::sync::Lazy;
use os_pipe::{PipeReader, PipeWriter};
use ouisync_lib::{StateMonitor, TracingLayer};
use std::{
    ffi::{CStr, CString},
    io::{self, BufRead, BufReader, Write},
    os::{raw, unix::io::AsRawFd},
    panic::{self, PanicInfo},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    thread::JoinHandle,
};

// Android log tag.
// HACK: if the tag doesn't start with 'flutter' then the logs won't show up in the app if built in
// release mode.
const TAG: &str = "flutter-ouisync";

static TRACING_LAYER: Lazy<TracingLayer> = Lazy::new(TracingLayer::new);

pub struct Logger {
    _stdout: StdRedirect,
    _stderr: StdRedirect,
}

impl Logger {
    pub(crate) fn new(trace_monitor: Option<StateMonitor>) -> Result<Self, io::Error> {
        // This should be set up before `setup_logger` is called, otherwise we won't see
        // `println!`s from inside the TracingLayer. Not really sure why that's the case though.
        let stdout = StdRedirect::new(io::stdout(), ANDROID_LOG_DEBUG)?;
        let stderr = StdRedirect::new(io::stderr(), ANDROID_LOG_ERROR)?;

        panic::set_hook(Box::new(panic_hook));
        setup_logger(trace_monitor);

        Ok(Self {
            _stdout: stdout,
            _stderr: stderr,
        })
    }
}

impl Drop for Logger {
    fn drop(&mut self) {
        TRACING_LAYER.set_monitor(None);
    }
}

// Redirect stdout or stderr into android log.
struct StdRedirect {
    handle: Option<JoinHandle<()>>,
    active: Arc<AtomicBool>,
    writer: PipeWriter,
    old_fd: libc::c_int,
    old_fd_dup: libc::c_int,
}

impl StdRedirect {
    fn new<T: AsRawFd>(stream: T, priority: LogPriority) -> Result<Self, io::Error> {
        let (reader, writer) = os_pipe::pipe()?;

        // Remember these so we can point the old stream FD to where it pointed before.
        //
        // SAFETY: The file descriptor should be valid because it was obtained using
        // `as_raw_fd` from a valid rust io object.
        let (old_fd, old_fd_dup) = unsafe {
            let old_fd = stream.as_raw_fd();
            let old_fd_dup = libc::dup(old_fd);
            (old_fd, old_fd_dup)
        };

        // SAFETY: Both file descriptors should be valid because they are obtained using
        // `as_raw_fd` from valid rust io objects.
        unsafe {
            if libc::dup2(writer.as_raw_fd(), stream.as_raw_fd()) < 0 {
                return Err(io::Error::last_os_error());
            }
        }

        let active = Arc::new(AtomicBool::new(true));
        let handle = thread::spawn({
            let active = active.clone();
            move || run(priority, reader, active)
        });

        Ok(Self {
            handle: Some(handle),
            active,
            writer,
            old_fd,
            old_fd_dup,
        })
    }
}

impl Drop for StdRedirect {
    fn drop(&mut self) {
        // Let the thread know we're done
        self.active.store(false, Ordering::Release);

        // Write empty line to the pipe to wake up the reader
        self.writer.write_all(b"\n").unwrap_or(());
        self.writer.flush().unwrap_or(());

        unsafe {
            // Point the original FD to it's previous target.
            let r = libc::dup2(self.old_fd_dup, self.old_fd);
            if r < 0 {
                print(
                    ANDROID_LOG_FATAL,
                    format!(
                        "Failed to point the redirected file descriptor \
                            to its original target (error code:{r})",
                    ),
                );
            }
        }

        if let Some(handle) = self.handle.take() {
            handle.join().unwrap_or(());
        }
    }
}

fn run(priority: LogPriority, reader: PipeReader, active: Arc<AtomicBool>) {
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    while active.load(Ordering::Acquire) {
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
        __android_log_print(priority as raw::c_int, TAG_C.as_ptr(), message.as_ptr());
    }
}

fn setup_logger(trace_monitor: Option<StateMonitor>) {
    use paranoid_android::{AndroidLogMakeWriter, Buffer};
    use tracing_subscriber::{
        filter::{LevelFilter, Targets},
        fmt,
        layer::SubscriberExt,
        util::SubscriberInitExt,
        Layer,
    };

    TRACING_LAYER.set_monitor(trace_monitor);

    tracing_subscriber::registry()
        .with(
            TRACING_LAYER
                .clone()
                .with_filter(Targets::new().with_target("ouisync", LevelFilter::TRACE)),
        )
        .with(
            fmt::layer()
                .pretty()
                .with_writer(AndroidLogMakeWriter::with_buffer(
                    TAG.to_owned(),
                    Buffer::Main,
                ))
                .with_target(false)
                .with_file(true)
                .with_line_number(true)
                .with_filter(
                    Targets::new()
                        // show logs from ouisync
                        .with_target("ouisync", LevelFilter::DEBUG)
                        // show DHT routing table stats
                        .with_target("btdht::routing", LevelFilter::DEBUG),
                ),
        )
        .try_init()
        // `Err` here just means the logger is already initialized, it's OK to ignore it.
        .unwrap_or(())
}

// Print panic messages to the andoid log as well.
fn panic_hook(info: &PanicInfo) {
    let message = match (info.payload().downcast_ref::<&str>(), info.location()) {
        (Some(message), Some(location)) => format!(
            "panic '{}' at {}:{}:{}",
            message,
            location.file(),
            location.line(),
            location.column(),
        ),
        (Some(message), None) => format!("panic '{message}'"),
        (None, Some(location)) => format!(
            "panic at {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        ),
        (None, None) => "panic".to_string(),
    };

    print(ANDROID_LOG_FATAL, message);
}
