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
    str::{self},
    sync::LazyLock,
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
const CONFIG_DIR: &str = "config";
const API_SOCKET: &str = "api.sock";
const DEFAULT_REPO: &str = "test";

static CERT: LazyLock<rcgen::CertifiedKey> =
    LazyLock::new(|| rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap());

impl Bin {
    /// Start ouisync as server
    pub fn start() -> Self {
        let id = Id::new();
        let base_dir = TempDir::new().unwrap();
        let config_dir = base_dir.path().join(CONFIG_DIR);
        let socket_path = base_dir.path().join(API_SOCKET);
        let store_dir = base_dir.path().join("store");
        let mount_dir = base_dir.path().join(MOUNT_DIR);

        fs::create_dir_all(mount_dir.join(DEFAULT_REPO)).unwrap();
        fs::create_dir_all(config_dir.join("root_certs")).unwrap();

        // Install the certificate
        let cert = CERT.cert.pem();

        // For server:
        fs::write(config_dir.join("cert.pem"), &cert).unwrap();
        fs::write(config_dir.join("key.pem"), CERT.key_pair.serialize_pem()).unwrap();

        // For client:
        fs::write(config_dir.join("root_certs").join("localhost.pem"), &cert).unwrap();

        let mut command = Command::new(COMMAND);
        command.arg("--socket").arg(&socket_path);
        command.arg("start");
        command.arg("--config-dir").arg(&config_dir);

        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let mut process = command.spawn().unwrap();

        let stdout = BufReader::new(process.stdout.take().unwrap());
        copy_lines_prefixed(stdout, OutputStream::Stdout, &id);

        let stderr = BufReader::new(process.stderr.take().unwrap());
        copy_lines_prefixed(stderr, OutputStream::Stderr, &id);

        wait_for_file_exists(&socket_path);

        let bin = Self {
            id,
            base_dir,
            process,
        };

        expect_output(
            &bin.id,
            "",
            bin.client_command()
                .arg("store-dir")
                .arg(store_dir)
                .output()
                .unwrap(),
        );

        expect_output(
            &bin.id,
            "",
            bin.client_command()
                .arg("mount-dir")
                .arg(mount_dir)
                .output()
                .unwrap(),
        );

        bin
    }

    pub fn root(&self) -> PathBuf {
        self.base_dir.path().join(MOUNT_DIR).join(DEFAULT_REPO)
    }

    #[track_caller]
    pub fn bind(&self) -> u16 {
        process_output(
            &self.id,
            self.client_command()
                .arg("bind")
                .arg(format!("tcp/{}:0", Ipv4Addr::LOCALHOST))
                .output()
                .unwrap(),
            |line| line.split(' ').nth(1)?.parse().ok(),
        )
    }

    #[track_caller]
    pub fn add_peer(&self, peer_port: u16) {
        expect_output(
            &self.id,
            "",
            self.client_command()
                .arg("add-peers")
                .arg(format!("tcp/{}:{peer_port}", Ipv4Addr::LOCALHOST))
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
            command.arg("--token").arg(share_token);
        } else {
            command.arg(DEFAULT_REPO);
        }

        expect_output(&self.id, "", command.output().unwrap());
    }

    /// Create a share token for the repository
    #[track_caller]
    pub fn share(&self) -> String {
        process_output(
            &self.id,
            self.client_command()
                .arg("share")
                .arg(DEFAULT_REPO)
                .arg("--mode")
                .arg("write")
                .output()
                .unwrap(),
            |line| Some(line.to_owned()),
        )
    }

    #[track_caller]
    pub fn mount(&self) {
        expect_output(
            &self.id,
            "",
            self.client_command().arg("mount").output().unwrap(),
        )
    }

    #[track_caller]
    pub fn enable_remote_control(&self) -> u16 {
        let addr: SocketAddr = str::from_utf8(
            &self
                .client_command()
                .arg("remote-control")
                .arg(format!("{}:0", Ipv4Addr::LOCALHOST))
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .parse()
        .unwrap();

        addr.port()
    }

    #[track_caller]
    pub fn mirror(&self, host: &str) {
        expect_output(
            &self.id,
            "",
            self.client_command()
                .arg("mirror")
                .arg("create")
                .arg(DEFAULT_REPO)
                .arg(host)
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
    static NEXT_ID: Cell<u32> = const { Cell::new(0) };
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
fn expect_output(id: &Id, expected: &str, output: Output) {
    if !output.status.success() {
        fail(id, output);
    }

    assert_eq!(str::from_utf8(&output.stdout).map(str::trim), Ok(expected));
}

#[track_caller]
fn process_output<F, R>(id: &Id, output: Output, f: F) -> R
where
    F: FnMut(&str) -> Option<R>,
{
    if !output.status.success() {
        fail(id, output);
    }

    str::from_utf8(&output.stdout)
        .unwrap()
        .lines()
        .find_map(f)
        .unwrap()
}

#[track_caller]
fn print_output(id: &Id, output: &[u8]) {
    let lines = str::from_utf8(output).unwrap().lines();

    for line in lines {
        println!("[{id}]     {line}");
    }
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
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn terminate(process: &Child) {
    // SAFETY: we are just sending a `SIGTERM` signal to the process, there should be no reason for
    // undefined behaviour here.
    unsafe {
        libc::kill(process.id() as libc::pid_t, libc::SIGTERM);
    }
}

#[cfg(target_os = "windows")]
fn terminate(_process: &Child) {
    todo!()
}

#[cfg(target_os = "android")]
fn terminate(_process: &Child) {
    todo!()
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
