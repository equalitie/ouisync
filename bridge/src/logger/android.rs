use super::redirect::{AsDescriptor, Redirect};
use ndk_sys::{
    __android_log_print, android_LogPriority as LogPriority,
    android_LogPriority_ANDROID_LOG_DEBUG as ANDROID_LOG_DEBUG,
    android_LogPriority_ANDROID_LOG_ERROR as ANDROID_LOG_ERROR,
    android_LogPriority_ANDROID_LOG_FATAL as ANDROID_LOG_FATAL,
};
use once_cell::sync::Lazy;
use os_pipe::PipeWriter;
use std::{
    ffi::{CStr, CString},
    io::{self, BufRead, BufReader, Stderr, Stdout},
    os::raw,
    panic::{self, PanicInfo},
    path::Path,
    process::{Child, Command, Stdio},
    thread,
};

// Android log tag.
// HACK: if the tag doesn't start with 'flutter' then the logs won't show up in the app if built in
// release mode.
const TAG: &str = "flutter-ouisync";

pub struct Logger {
    _stdout: Redirect<Stdout, PipeWriter>,
    _stderr: Redirect<Stderr, PipeWriter>,
}

impl Logger {
    pub(crate) fn new() -> Result<Self, io::Error> {
        let stdout = redirect(io::stdout(), ANDROID_LOG_DEBUG)?;
        let stderr = redirect(io::stderr(), ANDROID_LOG_ERROR)?;

        panic::set_hook(Box::new(panic_hook));
        setup_logger();

        Ok(Self {
            _stdout: stdout,
            _stderr: stderr,
        })
    }
}

pub struct Capture {
    process: Child,
}

impl Capture {
    pub fn new(path: &Path) -> io::Result<Self> {
        let mut rotate = super::create_rotate(path)?;

        let mut command = Command::new("logcat");
        command
            .args(["-vraw", "*:S", "flutter:V"])
            .arg(format!("{TAG}:V"))
            .stdout(Stdio::piped());

        // HACK: Spawning the logcat process in a separate thread because trying to spawn it in the
        // main thread causes hang on android for some reason.
        let mut process = thread::spawn(move || command.spawn())
            .join()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "panic when spawning logcat"))??;

        // Pipe the logcat output to the log file
        let mut stdout = process
            .stdout
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "failed to take logcat output"))?;
        thread::spawn(move || io::copy(&mut stdout, &mut rotate).ok());

        Ok(Self { process })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.process.kill().ok();
    }
}

fn redirect<S: AsDescriptor>(
    stream: S,
    priority: LogPriority,
) -> Result<Redirect<S, PipeWriter>, io::Error> {
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
        __android_log_print(priority as raw::c_int, TAG_C.as_ptr(), message.as_ptr());
    }
}

fn setup_logger() {
    use paranoid_android::{AndroidLogMakeWriter, Buffer};
    use tracing_subscriber::{
        filter::{LevelFilter, Targets},
        fmt,
        layer::SubscriberExt,
        util::SubscriberInitExt,
        Layer,
    };

    tracing_subscriber::registry()
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
                .with_filter(LevelFilter::DEBUG),
        )
        .with(
            Targets::new()
                .with_target("ouisync", LevelFilter::TRACE)
                .with_target("btdht::routing", LevelFilter::DEBUG)
                .with_target("sqlx", LevelFilter::WARN),
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
