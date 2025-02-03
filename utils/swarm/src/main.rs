use anyhow::{format_err, Result};
use clap::{value_parser, Parser};
use std::{
    ffi::OsStr,
    fs,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::mpsc,
    thread,
    time::Duration,
};

const REPO_NAME: &str = "repo";
const REPO_SHARE_TOKEN_W: &str =
    "https://ouisync.net/r#AwIgzc1v57theh0Vfc7tEsu_KLcoYwXvHBTJKUK6_XcOORM";
const REPO_SHARE_TOKEN_R: &str = "https://ouisync.net/r#AwEgH4o1Hl--wBhRXosR5I0fjkRZVsfCgnrnpRmzUSzDszogd31Fu6RF3hwgReJlsTimBMvPaJdPNc2rd23INs3goBk";

fn main() -> Result<()> {
    let options = Options::parse();

    build()?;

    let (tx, rx) = mpsc::sync_channel(0);
    ctrlc::set_handler(move || {
        tx.send(()).ok();
    })?;

    let mut children = Vec::new();

    for index in 0..options.writer_count {
        let child = spawn(index, Mode::Writer, &options.working_directory);
        children.push(child);
    }

    for index in 0..options.reader_count {
        let child = spawn(index, Mode::Reader, &options.working_directory);
        children.push(child);
    }

    rx.recv().ok();

    for mut child in children {
        terminate(&child);
        child.wait().unwrap();
    }

    Ok(())
}

fn spawn(index: u64, mode: Mode, working_directory: &Path) -> Child {
    let name = make_name(index, mode);
    let working_directory = working_directory.to_owned();
    let (tx, rx) = mpsc::sync_channel(0);

    thread::spawn(move || {
        if let Err(error) = Replica::new(name.clone(), mode, working_directory, tx).run() {
            eprintln!("[{}] {}", name, error);
        }
    });

    rx.recv().unwrap()
}

/// Utility to run swarm of ouisync replicas on the local machine, for testing.
///
/// All the replicas share a single repository which is initially empty and is wiped out before
/// each start. The repositories are mounted in `$WORKING_DIRECTORY/mnt/$REPLICA`. The replicas
/// connect to each other using local discovery.
///
/// Logs of all replicas are aggregated and printed to stdout. Each line is prefixed by the
/// identifier of the replica that produced it.
#[derive(Debug, Parser)]
struct Options {
    /// Number of writer replicas to spawn
    #[arg(
        short,
        long,
        value_parser = value_parser!(u64).range(0..),
        default_value_t = 0,
        value_name = "NUMBER",
    )]
    writer_count: u64,

    /// Number of reader replicas to spawn
    #[arg(
        short,
        long,
        value_parser = value_parser!(u64).range(0..),
        default_value_t = 0,
        value_name = "NUMBER",
    )]
    reader_count: u64,

    /// Working directory. All files (config, repo databases, sockets) of all replicas are placed
    /// here. Configs and databases are wiped out before start.
    #[arg(short = 'd', long)]
    working_directory: PathBuf,
}

fn build() -> Result<()> {
    let status = Command::new("cargo")
        .arg("build")
        .arg("--package")
        .arg("ouisync-cli")
        .arg("--release")
        // Adds debugging payload to the messages.
        .arg("--features")
        .arg("ouisync-lib/analyze-protocol")
        .status()?;

    if !status.success() {
        return Err(format_err!("Build failed"));
    }

    Ok(())
}

struct Replica {
    name: String,
    mode: Mode,
    working_directory: PathBuf,
    child_tx: mpsc::SyncSender<Child>,
    socket: PathBuf,
}

impl Replica {
    fn new(
        name: String,
        mode: Mode,
        working_directory: PathBuf,
        child_tx: mpsc::SyncSender<Child>,
    ) -> Self {
        let socket = working_directory
            .join("socks")
            .join(&name)
            .with_extension("sock");

        Self {
            name,
            mode,
            working_directory,
            child_tx,
            socket,
        }
    }

    fn run(self) -> Result<()> {
        fs::create_dir_all(self.socket.parent().unwrap())?;

        let config_path = self.working_directory.join("config").join(&self.name);
        remove_dir_all_if_exists(&config_path)?;
        fs::create_dir_all(&config_path)?;

        let store_path = self.working_directory.join("store").join(&self.name);
        remove_dir_all_if_exists(&store_path)?;
        fs::create_dir_all(&store_path)?;

        let (reader, stdout_writer) = os_pipe::pipe()?;
        let stderr_writer = stdout_writer.try_clone()?;

        let child = self
            .command()
            .arg("--config-dir")
            .arg(config_path)
            .arg("--store-dir")
            .arg(store_path)
            .arg("--log-color")
            .arg("always")
            .arg("start")
            .stdout(stdout_writer)
            .stderr(stderr_writer)
            .spawn()?;

        self.child_tx.send(child).unwrap();

        // Spawn a thread that captures the output of the child process and prefixes each of its
        // lines with the child name and then pipes it to the stdout of the main process.
        let name = self.name.clone();
        let output_handle = thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            let mut writer = io::stdout();
            let mut line = String::new();

            loop {
                line.clear();
                if reader.read_line(&mut line)? == 0 {
                    break;
                }

                write!(writer, "[{:4}] {}", name, line)?;
            }

            Ok::<_, anyhow::Error>(())
        });

        while !self.socket.try_exists()? {
            thread::sleep(Duration::from_millis(50));
        }

        self.client().arg("bind").arg("quic/0.0.0.0:0").run()?;
        self.client().arg("local-discovery").arg("on").run()?;

        // Create the repo unless it exists
        if !self
            .client()
            .arg("list-repositories")
            .run()?
            .lines()
            .any(|line| line == REPO_NAME)
        {
            self.client()
                .arg("create")
                .arg("--share-token")
                .arg(match self.mode {
                    Mode::Writer => REPO_SHARE_TOKEN_W,
                    Mode::Reader => REPO_SHARE_TOKEN_R,
                })
                .arg("--name")
                .arg(REPO_NAME)
                .run()?;
        }

        // Mount the repo
        let mount_path = self.working_directory.join("mnt").join(&self.name);
        fs::create_dir_all(&mount_path)?;
        self.client()
            .arg("mount")
            .arg("--name")
            .arg(REPO_NAME)
            .arg("--path")
            .arg(mount_path)
            .run()?;

        output_handle.join().unwrap()
    }

    fn command(&self) -> Command {
        let mut command = Command::new("target/release/ouisync");
        command.arg("--socket").arg(&self.socket);
        command
    }

    fn client(&self) -> Client {
        Client(self.command())
    }
}

struct Client(Command);

impl Client {
    fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.0.arg(arg);
        self
    }

    fn run(mut self) -> Result<String> {
        let output = self.0.output()?;

        if !output.status.success() {
            let name = self.0.get_program().to_string_lossy();
            let args = self
                .0
                .get_args()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");

            println!("{}", String::from_utf8_lossy(&output.stdout));
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));

            return Err(format_err!(
                "command `{} {}` failed with status: {}",
                name,
                args,
                output.status
            ));
        }

        Ok(String::from_utf8(output.stdout)?)
    }
}

fn make_name(index: u64, mode: Mode) -> String {
    format!(
        "{}{index}",
        match mode {
            Mode::Writer => "w",
            Mode::Reader => "r",
        }
    )
}

fn remove_dir_all_if_exists(path: impl AsRef<Path>) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[derive(Copy, Clone)]
enum Mode {
    Writer,
    Reader,
}

// Gracefully terminate the process, unlike `Child::kill` which sends `SIGKILL` and thus doesn't
// allow destructors to run.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
fn terminate(process: &Child) {
    // SAFETY: we are just sending a `SIGTERM` signal to the process, there should be no reason for
    // undefined behaviour here.
    unsafe {
        libc::kill(process.id() as libc::pid_t, libc::SIGTERM);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
fn terminate(_process: &Child) {
    todo!()
}
