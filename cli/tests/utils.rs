use std::{
    env,
    io::{self, BufRead, BufReader, Read, Write},
    net::SocketAddr,
    panic::{self, AssertUnwindSafe},
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
            .arg(format!("test:{}", mount_dir.path().display()));

        if let Some(share_token) = share_token {
            command.arg("--accept").arg(share_token);
        } else {
            command.arg("--share").arg("test");
        }

        command.arg("--print-ready-message");
        command.arg("--disable-upnp");
        command.arg("--disable-dht");
        command.arg("--disable-local-discovery");

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
    if let Some(line) = wait_for_line(process.stdout.as_mut().unwrap(), "ouisync:", "?name=test") {
        line
    } else {
        fail(process, id, "Failed to read share token");
    }
}

fn wait_for_ready_message(process: &mut Child, id: u32) -> u16 {
    const PREFIX: &str = "Listening on port ";
    if let Some(line) = wait_for_line(process.stdout.as_mut().unwrap(), PREFIX, "") {
        line[PREFIX.len()..].parse().unwrap()
    } else {
        fail(process, id, "Failed to read listening port");
    }
}

fn wait_for_line<R: Read>(reader: &mut R, prefix: &str, suffix: &str) -> Option<String> {
    let mut line = BufReader::new(reader)
        .lines()
        .filter_map(|line| line.ok())
        .find(|line| line.starts_with(prefix) && line.ends_with(suffix))?;

    let len = line.trim_end().len();
    line.truncate(len);
    Some(line)
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

/// Runs the given closure a couple of times until it succeeds (does not panic) with a short delay
/// between attempts. Panics (re-raising the last panic) if it doesn't succeedd even after all
/// atempts are exhausted.
#[track_caller]
pub fn eventually<F>(mut f: F)
where
    F: FnMut(),
{
    const ATTEMPTS: u32 = 10;
    const INITIAL_DELAY: Duration = Duration::from_millis(10);

    let mut last_panic_payload = None;

    // TODO: currently in case of panic, the panic message is printed multiple times (one for each
    // panickied attempt). This could in theory be supressed by setting an empty `panic_hook`
    // before the attempts and then restorring the original hook after them, before resuming the
    // panic. This however doesn't work in practice because `resume_unwind` does not invoke the
    // panic hook, so this would result in the program panicking, but without showing any message
    // which is not very useful. We should try to figure out a way to have the panic message printed
    // only once.

    for i in 0..ATTEMPTS {
        #[allow(clippy::redundant_closure)] // false positive
        match panic::catch_unwind(AssertUnwindSafe(|| f())) {
            Ok(()) => return,
            Err(payload) => {
                last_panic_payload = Some(payload);
            }
        }

        thread::sleep(INITIAL_DELAY * 2u32.pow(i));
    }

    if let Some(panic_payload) = last_panic_payload {
        panic::resume_unwind(panic_payload)
    } else {
        unreachable!()
    }
}

#[track_caller]
pub fn eventually_true<F>(mut f: F)
where
    F: FnMut() -> bool,
{
    const ATTEMPTS: u32 = 10;
    const INITIAL_DELAY: Duration = Duration::from_millis(10);

    for i in 0..ATTEMPTS {
        if f() {
            return;
        }

        thread::sleep(INITIAL_DELAY * 2u32.pow(i));
    }

    panic!("The test did not finish within the given timeout");
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
