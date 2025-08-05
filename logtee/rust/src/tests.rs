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
    let _lock = lock();
    let _log = init_log(false);
    let mut log_file = LogFile::new();

    let logtee = Logtee::start(log_file.path(), RotateOptions::default()).unwrap();

    tracing::debug!("first line");
    tracing::info!("second line");
    tracing::warn!("third line");

    log_file.expect_line(Level::DEBUG, "first line");
    log_file.expect_line(Level::INFO, "second line");
    log_file.expect_line(Level::WARN, "third line");

    drop(logtee);
    thread::sleep(Duration::from_millis(1));

    tracing::info!("this line is not captured");

    let _logtee = Logtee::start(log_file.path(), RotateOptions::default()).unwrap();

    tracing::info!("last line");

    log_file.expect_line(Level::INFO, "last line");
}

#[test]
fn stip_ansi() {
    let _lock = lock();
    let _log = init_log(true);
    let mut log_file = LogFile::new();

    let _logtee = Logtee::start(log_file.path(), RotateOptions::default()).unwrap();

    tracing::debug!("colored line");

    log_file.expect_line(Level::DEBUG, "colored line");
}

struct LogFile {
    tail: Tail,
    _temp_dir: TempDir,
}

impl LogFile {
    fn new() -> Self {
        let temp_dir = TempDir::new().unwrap();
        let tail = Tail::new(temp_dir.path().join("test.log"));

        Self {
            tail,
            _temp_dir: temp_dir,
        }
    }

    fn path(&self) -> &Path {
        self.tail.path()
    }

    #[track_caller]
    fn expect_line(&mut self, level: Level, message: &str) {
        let line = self.tail.next().unwrap().unwrap();
        assert_log_message(&line, level, message);
    }
}

// Allow only one test to run at a time. This is because these tests use a global resource and we
// don't want them to interfere with each other.
static MUTEX: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    MUTEX.lock().unwrap()
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
            let file = match &mut self.file {
                Some(file) => file,
                None => {
                    let mut file = wait_for_file(&self.path)?;
                    self.pos = file.seek(SeekFrom::Start(self.pos))?;
                    self.file.insert(BufReader::new(file))
                }
            };

            match file.read_line(&mut line) {
                Ok(n) if n > 0 => {
                    self.pos = file.get_mut().stream_position()?;
                    return Ok(line);
                }
                Ok(_) => {
                    self.file = None;
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

fn wait_for_file(path: &Path) -> io::Result<File> {
    loop {
        match File::open(path) {
            Ok(file) => return Ok(file),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                thread::sleep(SLEEP);
                continue;
            }
            Err(error) => return Err(error),
        }
    }
}

const SLEEP: Duration = Duration::from_millis(100);
