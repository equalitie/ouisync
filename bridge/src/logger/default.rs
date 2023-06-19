use os_pipe::PipeWriter;
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    thread,
};
use tracing_subscriber::{
    filter::{LevelFilter, Targets},
    fmt,
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter, Layer,
};

use super::redirect::Redirect;

pub struct Logger {
    _capture: Option<Capture>,
}

impl Logger {
    pub(crate) fn new(log_path: Option<PathBuf>) -> Result<Self, io::Error> {
        // Disable colors in output on Windows as `cmd` doesn't seem to support it.
        #[cfg(target_os = "windows")]
        let colors = false;
        #[cfg(not(target_os = "windows"))]
        let colors = true;

        tracing_subscriber::registry()
            .with(
                fmt::layer()
                    .pretty()
                    .with_ansi(colors)
                    .with_target(false)
                    .with_file(true)
                    .with_line_number(true)
                    .with_filter(
                        EnvFilter::builder()
                            // TODO: Allow changing the log level at runtime or at least at init
                            // time (via a command-line option or so)
                            .with_default_directive(LevelFilter::DEBUG.into())
                            .from_env_lossy(),
                    ),
            )
            .with(
                // This is a global filter which is fast. Filters can also be set on the individual
                // layers, but those are slower. So we want to filter out as much as possible here.
                Targets::new()
                    // everything from the ouisync crates
                    .with_target("ouisync", LevelFilter::TRACE)
                    // DHT routing events
                    .with_target("btdht::routing", LevelFilter::DEBUG)
                    // warnings from sqlx (they warn about slow queries among other things)
                    .with_target("sqlx", LevelFilter::WARN),
            )
            .try_init()
            // `Err` here just means the logger is already initialized, it's OK to ignore it.
            .unwrap_or(());

        // Capture output to the log file
        let capture = log_path.map(|path| Capture::new(&path)).transpose()?;

        Ok(Self { _capture: capture })
    }
}

/// Capture output (stdout and stderr) into the log file.
struct Capture {
    _stdout: Redirect<io::Stdout, PipeWriter>,
    _stderr: Redirect<io::Stderr, PipeWriter>,
}

impl Capture {
    fn new(path: &Path) -> io::Result<Self> {
        let rotate = super::create_rotate(path)?;

        // Print both to stdout/stderr and to log file:

        // Stdout
        let (mut reader, writer) = os_pipe::pipe()?;
        let stdout = Redirect::new(io::stdout(), writer)?;
        let stdout_orig = stdout.orig()?;

        thread::spawn(move || {
            io::copy(&mut reader, &mut FanOut(stdout_orig, rotate)).ok();
        });

        // Stderr
        let (mut reader, writer) = os_pipe::pipe()?;
        let stderr = Redirect::new(io::stderr(), writer)?;
        let stderr_orig = stderr.orig()?;

        thread::spawn(move || {
            io::copy(&mut reader, &mut FanOut(stderr_orig, io::stdout())).ok();
        });

        Ok(Self {
            _stdout: stdout,
            _stderr: stderr,
        })
    }
}

/// Writer that clones written data into two writers.
struct FanOut<A, B>(pub A, pub B);

impl<A, B> Write for FanOut<A, B>
where
    A: Write,
    B: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let len = self.0.write(buf)?;
        self.1.write_all(&buf[..len])?;

        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()?;
        self.1.flush()?;

        Ok(())
    }
}
