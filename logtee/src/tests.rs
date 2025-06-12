use std::{
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::PathBuf,
    sync::{Mutex, MutexGuard},
    thread,
    time::Duration,
};

use tempfile::TempDir;
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use super::*;

#[test]
fn sanity_check() {
    let mut test = Test::new(false);

    let logtee = Logtee::new(test.log_file_path(), RotateOptions::default());

    tracing::debug!("first line");
    tracing::info!("second line");
    tracing::warn!("third line");

    test.check_log_file_content(&[
        "DEBUG first line\n",
        " INFO second line\n",
        " WARN third line\n",
    ]);

    drop(logtee);

    tracing::info!("this line is not captured");

    let _logtee = Logtee::new(test.log_file_path(), RotateOptions::default());

    tracing::info!("last line");

    test.check_log_file_content(&[" INFO last line\n"]);
}

#[test]
fn stip_ansi() {
    let mut test = Test::new(true);
    let _logtee = Logtee::new(test.log_file_path(), RotateOptions::default());

    tracing::debug!("colored line");

    test.check_log_file_content(&["DEBUG colored line\n"]);
}

struct Test {
    _mutex: MutexGuard<'static, ()>,
    _tracing: DefaultGuard,
    log_file: Tail,
    _temp_dir: TempDir,
}

impl Test {
    fn new(ansi: bool) -> Self {
        // Allow only one test to run at a time. This is because these tests use a global resource
        // and we don't want them to interfere with each other.
        static MUTEX: Mutex<()> = Mutex::new(());

        let temp_dir = TempDir::new().unwrap();
        let log_file = Tail::new(temp_dir.path().join("test.log"));

        Self {
            _mutex: MUTEX.lock().unwrap(),
            _tracing: init_log(ansi),
            log_file,
            _temp_dir: temp_dir,
        }
    }

    fn log_file_path(&self) -> &Path {
        self.log_file.path()
    }

    #[track_caller]
    fn check_log_file_content(&mut self, expected: &[&str]) {
        for line in expected {
            assert_eq!(self.log_file.next().transpose().unwrap().unwrap(), *line);
        }
    }
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

// Iterator that tails a file (like `tail -F FILE`).
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
