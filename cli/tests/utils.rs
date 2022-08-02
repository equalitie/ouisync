use anyhow::{format_err, Error};
use backoff::{self, ExponentialBackoffBuilder};
use ouisync_lib::crypto::cipher::SecretKey;
use rand::Rng;
use std::{
    cell::Cell,
    env, fmt, fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::SocketAddr,
    path::PathBuf,
    process::{Child, ChildStdout, Command, Stdio},
    thread,
    time::Duration,
};
use tempfile::TempDir;

/// Wrapper for the ouisync binary.
pub struct Bin {
    id: Id,
    base_dir: TempDir,
    port: u16,
    share_token: String,
    process: Child,
}

const REPO_NAME: &str = "test";
const MOUNT_DIR: &str = "mnt";

impl Bin {
    pub fn start(peers: impl IntoIterator<Item = SocketAddr>, share_token: Option<&str>) -> Self {
        let id = Id::new();
        let base_dir = TempDir::new().unwrap();

        let mount_dir = base_dir.path().join(MOUNT_DIR);
        fs::create_dir(&mount_dir).unwrap();

        let mut command = Command::new(env!("CARGO_BIN_EXE_ouisync"));
        command.arg("--data-dir").arg(base_dir.path().join("data"));
        command
            .arg("--config-dir")
            .arg(base_dir.path().join("config"));
        command.arg("--bind").arg("tcp/127.0.0.1:0");
        command
            .arg("--mount")
            .arg(format!("{}:{}", REPO_NAME, mount_dir.display()));

        if let Some(share_token) = share_token {
            command.arg("--accept").arg(share_token);
        } else {
            command.arg("--create").arg(REPO_NAME);
            command.arg("--share").arg(format!("{}:write", REPO_NAME));
        }

        command.arg("--print-port");
        command.arg("--disable-upnp");
        command.arg("--disable-dht");
        command.arg("--disable-local-discovery");

        let master_key = SecretKey::random();
        command.arg("--key").arg(format!(
            "{}:{}",
            REPO_NAME,
            hex::encode(master_key.as_ref())
        ));

        for peer in peers {
            command.arg("--peers");
            command.arg(format!("tcp/{}", peer));
        }

        // Disable log output unless explicitly enabled.
        if env::var("RUST_LOG").is_err() {
            command.env("RUST_LOG", "off");
        }

        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let mut process = command.spawn().unwrap();

        let mut stdout = BufReader::new(process.stdout.take().unwrap());

        let share_token = if let Some(share_token) = share_token {
            share_token.to_owned()
        } else {
            wait_for_share_token(&mut process, &mut stdout, &id)
        };

        let port = wait_for_ready_message(&mut process, &mut stdout, &id);

        copy_lines_prefixed(stdout, io::stdout(), &id);

        let stderr = BufReader::new(process.stderr.take().unwrap());
        copy_lines_prefixed(stderr, io::stderr(), &id);

        Self {
            id,
            base_dir,
            port,
            share_token,
            process,
        }
    }

    pub fn root(&self) -> PathBuf {
        self.base_dir.path().join(MOUNT_DIR)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn share_token(&self) -> &str {
        &self.share_token
    }

    fn kill(&mut self) {
        terminate(&self.process);
        let exit_status = self.process.wait().unwrap();
        if !exit_status.success() {
            panic!("[{}] Process finished with {}", self.id, exit_status);
        }
    }
}

impl Drop for Bin {
    fn drop(&mut self) {
        self.kill();
    }
}

thread_local! {
    static NEXT_ID: Cell<u32> = Cell::new(0);
}

// Friendly id of a `Bin` used to prefix log messages to simplify debugging.
#[derive(Clone)]
struct Id {
    case: String,
    item: u32,
}

impl Id {
    fn new() -> Self {
        let thread = thread::current();
        let case = thread
            .name()
            .map(|name| name.to_owned())
            .unwrap_or_else(|| format!("{:?}", thread.id()));
        let item = NEXT_ID.with(|next_id| {
            let id = next_id.get();
            next_id.set(id + 1);
            id
        });

        Self { case, item }
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}/{}", self.case, self.item)
    }
}

// Spawns a thread that reads lines from `reader`, prefixes them with `id` and then writes them to
// `writer`.
fn copy_lines_prefixed<R, W>(mut reader: R, mut writer: W, id: &Id)
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    let id = id.clone();
    let mut line = String::new();

    thread::spawn(move || loop {
        line.clear();
        if reader.read_line(&mut line).unwrap() > 0 {
            write!(&mut writer, "[{}] {}", id, line).unwrap();
        } else {
            break;
        }
    });
}

fn wait_for_share_token(
    process: &mut Child,
    stdout: &mut BufReader<ChildStdout>,
    id: &Id,
) -> String {
    let suffix = format!("?name={}", REPO_NAME);
    if let Some(line) = wait_for_line(stdout, "https://ouisync.net/r", &suffix, id) {
        line
    } else {
        fail(process, id, "Failed to read share token");
    }
}

fn wait_for_ready_message(
    process: &mut Child,
    stdout: &mut BufReader<ChildStdout>,
    id: &Id,
) -> u16 {
    const PREFIX: &str = "Listening on TCP IPv4 port ";
    if let Some(line) = wait_for_line(stdout, PREFIX, "", id) {
        line[PREFIX.len()..].parse().unwrap()
    } else {
        fail(process, id, "Failed to read listening port");
    }
}

fn wait_for_line<R: BufRead>(
    reader: &mut R,
    prefix: &str,
    suffix: &str,
    id: &Id,
) -> Option<String> {
    for mut line in reader.lines().filter_map(|line| line.ok()) {
        if line.starts_with(prefix) && line.ends_with(suffix) {
            let len = line.trim_end().len();
            line.truncate(len);

            return Some(line);
        } else {
            println!("[{}] {}", id, line)
        }
    }

    None
}

fn fail(process: &mut Child, id: &Id, message: &str) -> ! {
    println!("[{}] {}", id, message);
    println!("[{}] Waiting for process to finish", id);

    let exit_status = process.wait().unwrap();

    println!("[{}] Process finished with {}", id, exit_status);
    println!("[{}] stderr:", id);

    let stderr = process.stderr.take().unwrap();

    for line in BufReader::new(stderr).lines() {
        println!("[{}]     {}", id, line.unwrap());
    }

    panic!("[{}] Failed to run ouisync executable", id);
}

/// Runs the given closure a couple of times until it succeeds (returns `Ok`) with a short delay
/// between attempts. Panics (with the last error) if it doesn't succeedd even after all atempts
/// are exhausted.
#[track_caller]
pub fn eventually<F>(f: F)
where
    F: FnMut() -> Result<(), Error>,
{
    eventually_with_timeout(Duration::from_secs(30), f)
}

#[track_caller]
pub fn eventually_with_timeout<F>(timeout: Duration, mut f: F)
where
    F: FnMut() -> Result<(), Error>,
{
    let backoff = ExponentialBackoffBuilder::new()
        .with_initial_interval(Duration::from_millis(100))
        .with_max_interval(Duration::from_secs(1))
        .with_randomization_factor(0.0)
        .with_multiplier(2.0)
        .with_max_elapsed_time(Some(timeout))
        .build();

    backoff::retry(backoff, || Ok(f()?)).unwrap()
}

pub fn check_eq<A, B>(a: A, b: B) -> Result<(), Error>
where
    A: PartialEq<B> + fmt::Debug,
    B: fmt::Debug,
{
    if a == b {
        Ok(())
    } else {
        Err(format_err!("check_eq failed: {:?} != {:?}", a, b))
    }
}

// Gracefully terminate the process, unlike `Child::kill` which sends `SIGKILL` and thus doesn't
// allow destructors to run.
// TODO: windows version
#[cfg(unix)]
fn terminate(process: &Child) {
    // SAFETY: we are just sending a `SIGTERM` signal to the process, there should be no reason for
    // undefined behaviour here.
    unsafe {
        libc::kill(process.id() as libc::pid_t, libc::SIGTERM);
    }
}

// RNG adaptor that implements `io::Read`.
pub struct RngRead<R>(pub R);

impl<R: Rng> Read for RngRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.fill(buf);
        Ok(buf.len())
    }
}

// `io::Write` that just counts the number of bytes written.
pub struct CountWrite(pub usize);

impl Write for CountWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
