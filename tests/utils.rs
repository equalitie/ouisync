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
    mount_dir: TempDir,
    port: u16,
    process: Child,
}

impl Bin {
    pub fn start(id: u32, peers: impl IntoIterator<Item = SocketAddr>) -> Self {
        let mount_dir = TempDir::new().unwrap();

        let mut command = Command::new(env!("CARGO_BIN_EXE_ouisync"));
        command.arg("--temp");
        command.arg("--mount");
        command.arg(format!("test:{}", mount_dir.path().display()));
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

        let mut stdout = process.stdout.take().unwrap();

        if let Some(port) = wait_for_ready_message(&mut stdout) {
            println!("replica {} ready on port {}", id, port);

            copy_lines_prefixed(stdout, io::stdout(), id);
            copy_lines_prefixed(process.stderr.take().unwrap(), io::stderr(), id);

            Self {
                mount_dir,
                port,
                process,
            }
        } else {
            println!("Failed to parse ready line.");
            println!("Waiting for program to finish.");

            let exit_status = process.wait();

            println!("Program finished with exit status: {:?}", exit_status);
            println!("stderr:");

            let stderr = process.stderr.take().unwrap();

            for line in BufReader::new(stderr).lines() {
                println!("    {:?}", line);
            }

            panic!("Failed to run ouisync executable");
        }
    }

    pub fn root(&self) -> &Path {
        self.mount_dir.path()
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    fn kill(&mut self) {
        self.process.kill().unwrap();
        self.process.wait().unwrap();
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

fn wait_for_ready_message<R: Read>(reader: &mut R) -> Option<u16> {
    const PREFIX: &str = "Listening on port ";

    BufReader::new(reader)
        .lines()
        .filter_map(|line| line.ok())
        .find(|line| line.starts_with(PREFIX))
        .map(|line| line[PREFIX.len()..].parse().unwrap())
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
