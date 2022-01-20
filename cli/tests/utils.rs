use anyhow::{format_err, Error};
use ouisync_lib::cipher::SecretKey;
use rand::Rng;
use std::{
    env,
    fmt::Debug,
    io::{self, BufRead, BufReader, Read, Write},
    net::SocketAddr,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};
use tempfile::TempDir;

/// Wrapper for the ouisync binary.
pub struct Bin {
    id: u32,
    mount_dir: TempDir,
    port: u16,
    share_token: String,
    process: Child,
}

const REPO_NAME: &str = "test";

impl Bin {
    pub fn start(
        id: u32,
        peers: impl IntoIterator<Item = SocketAddr>,
        share_token: Option<&str>,
    ) -> Self {
        let mount_dir = TempDir::new().unwrap();

        let mut command = Command::new(env!("CARGO_BIN_EXE_ouisync"));
        command.arg("--temp");
        command
            .arg("--mount")
            .arg(format!("{}:{}", REPO_NAME, mount_dir.path().display()));

        if let Some(share_token) = share_token {
            command.arg("--accept").arg(share_token);
        } else {
            command.arg("--create").arg(REPO_NAME);
            command.arg("--share").arg(format!("{}:write", REPO_NAME));
        }

        command.arg("--print-ready-message");
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
            command.arg(peer.to_string());
        }

        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let mut process = command.spawn().unwrap();

        let share_token = if let Some(share_token) = share_token {
            share_token.to_owned()
        } else {
            wait_for_share_token(&mut process, id)
        };

        let port = wait_for_ready_message(&mut process, id);

        copy_lines_prefixed(process.stdout.take().unwrap(), io::stdout(), id);
        copy_lines_prefixed(process.stderr.take().unwrap(), io::stderr(), id);

        Self {
            id,
            mount_dir,
            port,
            share_token,
            process,
        }
    }

    pub fn root(&self) -> &Path {
        self.mount_dir.path()
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
        println!("[{}] Process finished with {}", self.id, exit_status);
    }
}

impl Drop for Bin {
    fn drop(&mut self) {
        self.kill();
    }
}

// Spawns a thread that reads lines from `reader`, prefixes them with `id` and then writes them to
// `writer`.
fn copy_lines_prefixed<R, W>(reader: R, mut writer: W, id: u32)
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let mut reader = BufReader::new(reader);
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

fn wait_for_share_token(process: &mut Child, id: u32) -> String {
    let suffix = format!("?name={}", REPO_NAME);
    if let Some(line) = wait_for_line(process.stdout.as_mut().unwrap(), "ouisync:", &suffix, id) {
        line
    } else {
        fail(process, id, "Failed to read share token");
    }
}

fn wait_for_ready_message(process: &mut Child, id: u32) -> u16 {
    const PREFIX: &str = "Listening on port ";
    if let Some(line) = wait_for_line(process.stdout.as_mut().unwrap(), PREFIX, "", id) {
        line[PREFIX.len()..].parse().unwrap()
    } else {
        fail(process, id, "Failed to read listening port");
    }
}

fn wait_for_line<R: Read>(reader: &mut R, prefix: &str, suffix: &str, id: u32) -> Option<String> {
    for mut line in BufReader::new(reader).lines().filter_map(|line| line.ok()) {
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

fn fail(process: &mut Child, id: u32, message: &str) -> ! {
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
pub fn eventually<F>(mut f: F)
where
    F: FnMut() -> Result<(), Error>,
{
    const ATTEMPTS: u32 = 10;
    const INITIAL_DELAY: Duration = Duration::from_millis(10);

    let mut last_error = None;

    for i in 0..ATTEMPTS {
        match f() {
            Ok(()) => return,
            Err(error) => {
                last_error = Some(error);
            }
        }

        thread::sleep(INITIAL_DELAY * 2u32.pow(i));
    }

    if let Some(error) = last_error {
        panic!("{}", error)
    } else {
        unreachable!()
    }
}

pub fn check_eq<A, B>(a: A, b: B) -> Result<(), Error>
where
    A: PartialEq<B> + Debug,
    B: Debug,
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
