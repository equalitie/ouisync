use anyhow::{format_err, Result};
use clap::{value_parser, Parser};
use std::{
    ffi::OsStr,
    fs,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
    time::Duration,
};

const REPO_NAME: &str = "repo";
const REPO_SHARE_TOKEN: &str =
    "https://ouisync.net/r#AwIgzc1v57theh0Vfc7tEsu_KLcoYwXvHBTJKUK6_XcOORM";

fn main() -> Result<()> {
    let options = Options::parse();

    build()?;

    for index in 0..options.count {
        let name = make_name(index);
        let working_directory = options.working_directory.clone();

        thread::spawn(move || {
            if let Err(error) = Replica::new(name.clone(), working_directory).run() {
                eprintln!("[{}] {}", name, error);
            }
        });
    }

    let (tx, rx) = mpsc::sync_channel(0);
    ctrlc::set_handler(move || {
        tx.send(()).ok();
    })?;
    rx.recv().ok();

    Ok(())
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
    /// Number of replicas to spawn
    #[arg(
        short = 'c',
        long,
        value_parser = value_parser!(u64).range(1..),
        default_value_t = 1,
        value_name = "NUMBER",
    )]
    count: u64,

    /// Working directory. All files (config, repo databases, sockets) of all replicas are placed
    /// here. Configs and databases are wiped out before start.
    #[arg(short = 'w', long)]
    working_directory: PathBuf,
}

fn build() -> Result<()> {
    let status = Command::new("cargo")
        .arg("build")
        .arg("--package")
        .arg("ouisync-cli")
        .arg("--release")
        .status()?;

    if !status.success() {
        return Err(format_err!("Build failed"));
    }

    Ok(())
}

struct Replica {
    name: String,
    working_directory: PathBuf,
    socket: PathBuf,
}

impl Replica {
    fn new(name: String, working_directory: PathBuf) -> Self {
        let socket = working_directory
            .join("socks")
            .join(&name)
            .with_extension("sock");

        Self {
            name,
            working_directory,
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

        self.command()
            .arg("--config-dir")
            .arg(config_path)
            .arg("--store-dir")
            .arg(store_path)
            .arg("start")
            .arg("--log-color")
            .arg("always")
            .stdout(stdout_writer)
            .stderr(stderr_writer)
            .spawn()?;

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
                .arg(REPO_SHARE_TOKEN)
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

        let mut reader = BufReader::new(reader);
        let mut writer = io::stdout();
        let mut line = String::new();

        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                break;
            }

            write!(writer, "[{:4}] {}", self.name, line)?;
        }

        Ok(())
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

fn make_name(index: u64) -> String {
    format!("{index}")
}

fn remove_dir_all_if_exists(path: impl AsRef<Path>) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
