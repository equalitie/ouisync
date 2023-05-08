use anyhow::{format_err, Error};
use backoff::{self, ExponentialBackoffBuilder};
use rand::Rng;
use std::{
    cell::Cell,
    env, fmt, fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    str::{self, FromStr},
    thread,
    time::Duration,
};
use tempfile::TempDir;

/// Wrapper for the ouisync binary.
pub struct Bin {
    id: Id,
    base_dir: TempDir,
    process: Child,
}

const COMMAND: &str = env!("CARGO_BIN_EXE_ouisync");
const MOUNT_DIR: &str = "mnt";
const API_SOCKET: &str = "api.sock";
const DEFAULT_REPO: &str = "test";

impl Bin {
    /// Start ouisync as server
    pub fn start() -> Self {
        let id = Id::new();
        let base_dir = TempDir::new().unwrap();
        let socket_path = base_dir.path().join(API_SOCKET);
        let mount_dir = base_dir.path().join(MOUNT_DIR);

        fs::create_dir_all(mount_dir.join(DEFAULT_REPO)).unwrap();

        let mut command = Command::new(COMMAND);
        command
            .arg("--store-dir")
            .arg(base_dir.path().join("store"));
        command
            .arg("--config-dir")
            .arg(base_dir.path().join("config"));
        command.arg("--mount-dir").arg(&mount_dir);
        command.arg("--socket").arg(&socket_path);
        command.arg("start");

        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let mut process = command.spawn().unwrap();

        let stdout = BufReader::new(process.stdout.take().unwrap());
        copy_lines_prefixed(stdout, OutputStream::Stdout, &id);

        let stderr = BufReader::new(process.stderr.take().unwrap());
        copy_lines_prefixed(stderr, OutputStream::Stderr, &id);

        wait_for_file_exists(&socket_path);

        Self {
            id,
            base_dir,
            process,
        }
    }

    pub fn root(&self) -> PathBuf {
        self.base_dir.path().join(MOUNT_DIR).join(DEFAULT_REPO)
    }

    #[track_caller]
    pub fn bind(&self) {
        expect_output(
            &self.id,
            "OK",
            self.client_command()
                .arg("bind")
                .arg(format!("tcp/{}:0", Ipv4Addr::LOCALHOST))
                .output()
                .unwrap(),
        )
    }

    #[track_caller]
    pub fn get_port(&self) -> u16 {
        parse_prefixed_line(
            &self.id,
            "TCP, IPv4:",
            self.client_command().arg("list-ports").output().unwrap(),
        )
    }

    #[track_caller]
    pub fn add_peer(&self, peer_port: u16) {
        expect_output(
            &self.id,
            "OK",
            self.client_command()
                .arg("add-peers")
                .arg(&format!("tcp/{}:{peer_port}", Ipv4Addr::LOCALHOST))
                .output()
                .unwrap(),
        )
    }

    /// Create a repository
    #[track_caller]
    pub fn create(&self, share_token: Option<&str>) {
        let mut command = self.client_command();
        command.arg("create");

        if let Some(share_token) = share_token {
            command.arg("--share-token").arg(share_token);
        } else {
            command.arg("--name").arg(DEFAULT_REPO);
        }

        expect_output(&self.id, "OK", command.output().unwrap());
    }

    /// Create a share token for the repository
    #[track_caller]
    pub fn share(&self) -> String {
        parse_prefixed_line(
            &self.id,
            "",
            self.client_command()
                .arg("share")
                .arg("--name")
                .arg(DEFAULT_REPO)
                .arg("--mode")
                .arg("write")
                .output()
                .unwrap(),
        )
    }

    #[track_caller]
    pub fn mount(&self) {
        expect_output(
            &self.id,
            "OK",
            self.client_command()
                .arg("mount")
                .arg("--all")
                .output()
                .unwrap(),
        )
    }

    #[track_caller]
    pub fn bind_rpc(&self) -> u16 {
        let addr: SocketAddr = parse_prefixed_line(
            &self.id,
            "",
            self.client_command()
                .arg("bind-rpc")
                .arg(&format!("{}:0", Ipv4Addr::LOCALHOST))
                .output()
                .unwrap(),
        );

        addr.port()
    }

    #[track_caller]
    pub fn mirror(&self, mirror_port: u16) {
        expect_output(
            &self.id,
            "OK",
            self.client_command()
                .arg("mirror")
                .arg("--name")
                .arg(DEFAULT_REPO)
                .arg("--host")
                .arg(&format!("{}:{mirror_port}", Ipv4Addr::LOCALHOST))
                .output()
                .unwrap(),
        );
    }

    #[track_caller]
    fn kill(&mut self) {
        terminate(&self.process);
        let exit_status = self.process.wait().unwrap();
        if !exit_status.success() {
            panic!("[{}] Process finished with {}", self.id, exit_status);
        }
    }

    fn client_command(&self) -> Command {
        let mut command = Command::new(COMMAND);
        command
            .arg("--socket")
            .arg(self.base_dir.path().join(API_SOCKET));
        command
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

fn wait_for_file_exists(path: &Path) {
    // poor man's inotify :)
    loop {
        match path.try_exists() {
            Ok(true) => break,
            Ok(false) => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => panic!(
                "Failed to check existence of file '{}': {:?}",
                path.display(),
                error
            ),
        }
    }
}

enum OutputStream {
    Stdout,
    Stderr,
}

// Spawns a thread that reads lines from `reader`, prefixes them with `id` and then writes them to
// `writer`.
fn copy_lines_prefixed<R>(mut reader: R, output: OutputStream, id: &Id)
where
    R: BufRead + Send + 'static,
{
    let id = id.clone();
    let mut line = String::new();

    thread::spawn(move || loop {
        line.clear();
        if reader.read_line(&mut line).unwrap() > 0 {
            match output {
                OutputStream::Stdout => print!("[{id}] {line}"),
                OutputStream::Stderr => eprint!("[{id}] {line}"),
            }
        } else {
            break;
        }
    });
}

#[track_caller]
fn parse_prefixed_line<T>(id: &Id, prefix: &str, output: Output) -> T
where
    T: FromStr,
    T::Err: fmt::Debug,
{
    if !output.status.success() {
        fail(id, output);
    }

    let line = find_prefixed_line(id, prefix, &output.stdout);
    let line = line[prefix.len()..].trim();

    line.parse().unwrap()
}

#[track_caller]
fn expect_output(id: &Id, expected: &str, output: Output) {
    if !output.status.success() {
        fail(id, output);
    }

    assert_eq!(str::from_utf8(&output.stdout).map(str::trim), Ok(expected));
}

#[track_caller]
fn print_output(id: &Id, output: &[u8]) {
    let lines = str::from_utf8(output).unwrap().lines();

    for line in lines {
        println!("[{id}]     {line}");
    }
}

#[track_caller]
fn find_prefixed_line<'a>(id: &'_ Id, prefix: &'_ str, output: &'a [u8]) -> &'a str {
    let lines = str::from_utf8(output).unwrap().lines();

    for line in lines {
        if line.starts_with(prefix) {
            return line;
        } else {
            println!("[{id}] {line}");
        }
    }

    panic!("Output does not contain line prefixed with '{prefix}'");
}

fn fail(id: &Id, output: Output) -> ! {
    println!("[{id}] Process finished with {}", output.status);

    println!("[{id}] stdout:");
    print_output(id, &output.stdout);

    println!("[{id}] stderr:");
    print_output(id, &output.stderr);

    panic!("[{id}] Failed to run ouisync executable");
}

/// Runs the given closure a couple of times until it succeeds (returns `Ok`) with a short delay
/// between attempts. Panics (with the last error) if it doesn't succeedd even after all atempts
/// are exhausted.
#[track_caller]
pub fn eventually<F>(f: F)
where
    F: FnMut() -> Result<(), Error>,
{
    eventually_with_timeout(Duration::from_secs(60), f)
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
