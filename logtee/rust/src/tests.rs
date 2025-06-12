use std::{
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::PathBuf,
    sync::{Mutex, MutexGuard},
    thread,
    time::Duration,
};

use tempfile::TempDir;
use tracing::{subscriber::DefaultGuard, Level};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use super::*;

#[test]
fn sanity_check() {
    let mut env = Env::new(false);

    let logtee = Logtee::start(env.log_file_path(), RotateOptions::default()).unwrap();

    tracing::debug!("first line");
    tracing::info!("second line");
    tracing::warn!("third line");

    env.expect_log_file_line(Level::DEBUG, "first line");
    env.expect_log_file_line(Level::INFO, "second line");
    env.expect_log_file_line(Level::WARN, "third line");

    drop(logtee);
    thread::sleep(Duration::from_millis(1));

    tracing::info!("this line is not captured");

    let _logtee = Logtee::start(env.log_file_path(), RotateOptions::default()).unwrap();

    tracing::info!("last line");

    env.expect_log_file_line(Level::INFO, "last line");
}

#[test]
fn stip_ansi() {
    let mut env = Env::new(true);
    let _logtee = Logtee::start(env.log_file_path(), RotateOptions::default()).unwrap();

    tracing::debug!("colored line");

    env.expect_log_file_line(Level::DEBUG, "colored line");
}

// Test environment. Create one at the begining of each test.
struct Env {
    _mutex: MutexGuard<'static, ()>,
    _tracing: DefaultGuard,
    log_file: Tail,
    _temp_dir: TempDir,
}

impl Env {
    fn new(ansi: bool) -> Self {
        // Allow only one test to run at a time. This is because these tests use a global resource
        // and we don't want them to interfere with each other.
        static MUTEX: Mutex<()> = Mutex::new(());

        let temp_dir = TempDir::new().unwrap();
        let log_file = Tail::new(temp_dir.path().join("test.log"));

        Self {
            _mutex: MUTEX.lock().unwrap_or_else(|error| error.into_inner()),
            _tracing: init_log(ansi),
            log_file,
            _temp_dir: temp_dir,
        }
    }

    fn log_file_path(&self) -> &Path {
        self.log_file.path()
    }

    #[track_caller]
    fn expect_log_file_line(&mut self, level: Level, message: &str) {
        let line = self.log_file.next().unwrap().unwrap();
        assert_log_message(&line, level, message);
    }
}

#[track_caller]
fn assert_log_message(actual: &str, expected_level: Level, expected_message: &str) {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[track_caller]
    fn check(actual: &str, expected_level: Level, expected_message: &str) {
        assert_eq!(actual, format!("{expected_level:>5} {expected_message}\n"));
    }

    #[cfg(target_os = "android")]
    #[track_caller]
    fn check(actual: &str, expected_level: Level, expected_message: &str) {
        use regex::Regex;
        use std::process;

        let regex = Regex::new(
            r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3} \+0000) (\S)/(\S+)\s*\((\d+)\):\s*(.*)",
        )
        .unwrap();

        let captures = regex.captures(actual).unwrap();
        let (_, [_, actual_level, actual_tag, actual_pid, actual_message]) = captures.extract();

        let expected_level = match expected_level {
            Level::ERROR => "E",
            Level::WARN => "W",
            Level::INFO => "I",
            Level::DEBUG => "D",
            Level::TRACE => "V",
        };

        let actual = format!("{actual_level}/{actual_tag} ({actual_pid}): {actual_message}");
        let expected = format!(
            "{expected_level}/{} ({}): {expected_message}",
            env!("CARGO_PKG_NAME"),
            process::id()
        );

        assert_eq!(actual, expected);
    }

    check(actual, expected_level, expected_message);
}

fn init_log(ansi: bool) -> DefaultGuard {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    let layer = tracing_subscriber::fmt::layer();
    #[cfg(target_os = "android")]
    let layer = paranoid_android::layer(env!("CARGO_PKG_NAME"));

    tracing_subscriber::registry()
        .with(
            layer
                .without_time()
                .with_ansi(ansi)
                .with_target(false)
                .with_file(false)
                .with_line_number(false),
        )
        .set_default()
}

// Iterator that tails a file. Like `tail -F FILE` but always starts at the beginning of the file.
struct Tail {
    path: PathBuf,
    file: Option<BufReader<File>>,
    pos: u64,
}

impl Tail {
    const SLEEP: Duration = Duration::from_millis(100);

    fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            file: None,
            pos: 0,
        }
    }

    fn path(&self) -> &Path {
        self.path.as_path()
    }

    fn read(&mut self) -> io::Result<String> {
        let mut line = String::new();

        loop {
            let Some(file) = &mut self.file else {
                match File::open(&self.path) {
                    Ok(mut file) => {
                        self.pos = file.seek(SeekFrom::Start(self.pos))?;
                        self.file = Some(BufReader::new(file));
                        continue;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        thread::sleep(Self::SLEEP);
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            };

            match file.read_line(&mut line) {
                Ok(n) if n > 0 => {
                    self.pos = file.get_mut().stream_position()?;
                    return Ok(line);
                }
                Ok(_) => {
                    self.file = None;
                    thread::sleep(Self::SLEEP);
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl Iterator for Tail {
    type Item = io::Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.read())
    }
}
