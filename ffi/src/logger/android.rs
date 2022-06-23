use android_log_sys::{LogPriority, __android_log_print};
use android_logger::FilterBuilder;
use log::{Level, LevelFilter};
use once_cell::sync::Lazy;
use os_pipe::{PipeReader, PipeWriter};
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

pub(crate) struct Logger {
    _stdout: StdRedirect,
    _stderr: StdRedirect,
}

impl Logger {
    pub fn new() -> Result<Self, io::Error> {
        panic::set_hook(Box::new(panic_hook));
        setup_logger();

        Ok(Self {
            _stdout: StdRedirect::new(io::stdout(), LogPriority::DEBUG)?,
            _stderr: StdRedirect::new(io::stderr(), LogPriority::ERROR)?,
        })
    }
}

// Redirect stdout or stderr into android log.
struct StdRedirect {
    handle: Option<JoinHandle<()>>,
    active: Arc<AtomicBool>,
    writer: PipeWriter,
}

impl StdRedirect {
    fn new<T: AsRawFd>(stream: T, priority: LogPriority) -> Result<Self, io::Error> {
        let (reader, writer) = os_pipe::pipe()?;

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
        })
    }
}

impl Drop for StdRedirect {
    fn drop(&mut self) {
        // FIXME: potential race condition here - the atomic store should happen before the write

        // Write empty line to the pipe to wake up the reader
        self.writer.write_all(b"\n").unwrap_or(());
        self.writer.flush().unwrap_or(());

        self.active.store(false, Ordering::Release);

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
                print(LogPriority::ERROR, error.to_string());
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

fn setup_logger() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag(TAG)
            .with_min_level(Level::Trace)
            .with_filter(
                FilterBuilder::new()
                    // disable logs from dependencies to avoid log spam
                    .filter(None, LevelFilter::Off)
                    // show logs from ouisync-ffi
                    .filter(Some(env!("CARGO_PKG_NAME")), LevelFilter::Trace)
                    // show logs from ouisync
                    .filter(Some("ouisync"), LevelFilter::Trace)
                    // show DHT routing table stats
                    .filter(Some("btdht::routing"), LevelFilter::Debug)
                    .build(),
            )
            .format(|f, record| {
                write!(
                    f,
                    "{} ({}:{})",
                    record.args(),
                    record.file().unwrap_or("?"),
                    record.line().unwrap_or(0)
                )
            }),
    )
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
        (Some(message), None) => format!("panic '{}'", message),
        (None, Some(location)) => format!(
            "panic at {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        ),
        (None, None) => "panic".to_string(),
    };

    print(LogPriority::FATAL, message);
}
