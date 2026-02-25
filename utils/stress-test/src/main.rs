use clap::{Parser, value_parser};
use serde::Deserialize;
use std::{
    borrow::Cow,
    env, fmt,
    io::{self, BufRead, BufReader, Read, Write},
    mem,
    process::{self, Child, ChildStderr, ChildStdout, Command, Stdio},
    sync::mpsc::{self, RecvTimeoutError},
    thread::{self},
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn main() {
    let options = Options::parse();

    // Override TMPDIR so that all temp files/directories created in the tests are created inside
    // it, for easier cleanup.
    let temp_root = TempDir::new().unwrap();

    // SAFETY: This is the only thread accessing these env vars at this moment.
    unsafe {
        env::set_var("TMPDIR", temp_root.path()); // unix
        env::set_var("TEMP", temp_root.path()); // windows
    }

    // Build the test binaries
    let exes = if let Some(exe) = options.exe {
        vec![exe]
    } else {
        build(&options)
    };

    let args: Vec<_> = options
        .exact
        .then_some("--exact")
        .into_iter()
        .chain(options.skip.iter().flat_map(|filter| ["--skip", filter]))
        .chain(options.filters.iter().map(|filter| filter.as_str()))
        .map(|s| s.to_owned())
        .collect();

    let args_display = args.join(" ");

    let runner = options.runner.or_else(get_runner_from_env);

    println!(
        "Starting {} {}:",
        options.concurrency,
        if options.concurrency == 1 {
            "process"
        } else {
            "processes"
        }
    );

    let exe_prefix = if let Some(runner) = &runner {
        format!("{runner} ")
    } else {
        String::new()
    };

    for exe in &exes {
        println!("    {exe_prefix}{exe} {args_display}",);
    }

    let (tx, rx) = mpsc::sync_channel(0);

    for index in 0..options.concurrency {
        thread::spawn({
            let runner = runner.clone();
            let exes = exes.clone();
            let args = args.clone();
            let tx = tx.clone();
            move || run(index, runner, exes, args, tx)
        });
    }

    let start = Instant::now();

    for (global_iteration, progress) in rx.into_iter().enumerate() {
        print!(
            "{} #{} ({}/{})",
            DisplayDuration(start.elapsed()),
            global_iteration,
            progress.process,
            progress.iteration,
        );

        match progress.status {
            Status::Success => println!(),
            Status::Failure {
                stdout,
                stderr,
                code,
            } => {
                print!("\tFAILURE (");
                if let Some(code) = code {
                    print!("exit code: {code}");
                } else {
                    print!("terminated by signal")
                }
                println!(")");

                println!("\n\n---- stdout: ----\n\n");
                io::stdout().write_all(&stdout).unwrap();

                println!("\n\n---- stderr: ----\n\n");
                io::stderr().write_all(&stderr).unwrap();

                break;
            }
            Status::Slow {
                elapsed,
                stdout,
                stderr,
            } => {
                println!("\ttaking too long (so far: {})", DisplayDuration(elapsed));
                println!("\noutput since last report:");

                println!("\n\n---- stdout: ----\n\n");
                io::stdout().write_all(&stdout).unwrap();

                println!("\n\n---- stderr: ----\n\n");
                io::stderr().write_all(&stderr).unwrap();
            }
        }
    }
}

#[derive(Debug, Parser)]
struct Options {
    /// Number of processes to run concurrently
    #[arg(
        short = 'C',
        long,
        value_parser = value_parser!(u64).range(1..),
        default_value_t = 1,
        value_name = "NUMBER",
    )]
    concurrency: u64,

    /// Package to build
    #[arg(short, long, default_value = "ouisync")]
    package: String,

    /// Space or comma separated list of features to activate
    #[arg(short = 'F', long)]
    features: Vec<String>,

    /// Build package in release mode
    #[arg(short, long)]
    release: bool,

    /// Test only this package's library
    #[arg(long)]
    lib: bool,

    /// Test only the specified test target
    #[arg(long, value_name = "NAME")]
    test: Vec<String>,

    /// Test the given executable instead of building it from source
    #[arg(long, value_name = "PATH", conflicts_with_all = ["package", "features", "release", "lib", "test"])]
    exe: Option<String>,

    /// Skip tests whose names contain FILTER
    #[arg(long, value_name = "FILTER")]
    skip: Vec<String>,

    /// Exactly match filters rather than by substring
    #[arg(long)]
    exact: bool,

    /// If provided, test executables are executed using this runner with the test executable passed
    /// as an argument.
    #[arg(long, value_name = "PATH")]
    runner: Option<String>,

    /// Run only tests whose names contain FILTER
    filters: Vec<String>,
}

fn build(options: &Options) -> Vec<String> {
    let mut command = Command::new("cargo");

    command
        .arg("test")
        .arg("--no-run")
        .arg("--package")
        .arg(&options.package)
        .arg("--message-format")
        .arg("json");

    if options.release {
        command.arg("--release");
    }

    for feature in &options.features {
        command.arg("--features").arg(feature);
    }

    if options.lib {
        command.arg("--lib");
    }

    for test in &options.test {
        command.arg("--test").arg(test);
    }

    let display_command = command.get_program().to_str().unwrap().to_owned();
    let display_args = command
        .get_args()
        .map(|arg| arg.to_str().unwrap())
        .collect::<Vec<_>>()
        .join(" ");

    println!("Running `{display_command} {display_args}`",);

    command.stderr(Stdio::inherit());

    let output = command.output().unwrap();

    if !output.status.success() {
        println!("Build failed");
        process::exit(1);
    }

    let stdout = BufReader::new(&output.stdout[..]);
    stdout
        .lines()
        .map(|line| serde_json::from_str::<BuildMessage>(&line.unwrap()).unwrap())
        .filter(|message| {
            message
                .target
                .as_ref()
                .map(|target| {
                    target.kind.contains(&BuildMessageTargetKind::Lib)
                        || target.kind.contains(&BuildMessageTargetKind::Test)
                })
                .unwrap_or(false)
        })
        .filter_map(|message| message.executable.map(|exe| exe.into_owned()))
        .collect()
}

#[derive(Deserialize)]
struct BuildMessage<'a> {
    executable: Option<Cow<'a, str>>,
    target: Option<BuildMessageTarget>,
}

#[derive(Deserialize)]
struct BuildMessageTarget {
    kind: Vec<BuildMessageTargetKind>,
}

#[derive(Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum BuildMessageTargetKind {
    Bin,
    Cdylib,
    CustomBuild,
    Lib,
    Rlib,
    Staticlib,
    Test,
    ProcMacro,
}

fn run(
    process: u64,
    runner: Option<String>,
    exes: Vec<String>,
    args: Vec<String>,
    tx: mpsc::SyncSender<Progress>,
) {
    let mut commands: Vec<_> = exes
        .into_iter()
        .map(|exe| {
            let mut command = if let Some(runner) = &runner {
                let mut command = Command::new(runner);
                command.arg(exe);
                command
            } else {
                Command::new(exe)
            };

            command.args(&args);
            command
        })
        .collect();

    let runner = CommandRunner::new();

    for iteration in 0.. {
        for command in &mut commands {
            let mut running = runner.run(command);

            loop {
                match running.next(Duration::from_mins(1)) {
                    Status::Success => break,
                    status @ Status::Failure { .. } => {
                        tx.send(Progress {
                            process,
                            iteration,
                            status,
                        })
                        .ok();
                        return;
                    }
                    status @ Status::Slow { .. } => {
                        if tx
                            .send(Progress {
                                process,
                                iteration,
                                status,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
        }

        if tx
            .send(Progress {
                process,
                iteration,
                status: Status::Success,
            })
            .is_err()
        {
            break;
        }
    }
}

struct Progress {
    process: u64,
    iteration: u64,
    status: Status,
}

struct DisplayDuration(Duration);

impl fmt::Display for DisplayDuration {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = self.0.as_secs();
        let m = s / 60;
        let h = m / 60;

        let s = s - m * 60;
        let m = m - h * 60;

        write!(f, "{h:02}:{m:02}:{s:02}")
    }
}

fn get_runner_from_env() -> Option<String> {
    #[allow(unreachable_patterns)]
    match true {
        cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )) => env::var("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER").ok(),
        cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "musl"
        )) => env::var("CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUNNER").ok(),
        // TODO: other targets
        _ => None,
    }
}

struct CommandRunner {
    stdout_tx: mpsc::SyncSender<(ChildStdout, mpsc::SyncSender<Chunk>)>,
    stderr_tx: mpsc::SyncSender<(ChildStderr, mpsc::SyncSender<Chunk>)>,
}

impl CommandRunner {
    fn new() -> Self {
        fn forward_output<R: Read>(
            buffer: Buffer,
            rx: mpsc::Receiver<(R, mpsc::SyncSender<Chunk>)>,
        ) {
            let mut chunk = [0; 1024];
            for (mut reader, status_tx) in rx {
                loop {
                    match reader.read(&mut chunk) {
                        Ok(n @ 1..) => {
                            status_tx.send((buffer, chunk[..n].to_vec())).ok();
                        }
                        Ok(0) => break,
                        Err(error) => {
                            eprintln!("failed to read from process output: {error:?}");
                            break;
                        }
                    }
                }
            }
        }

        let (stdout_tx, stdout_rx) = mpsc::sync_channel(1);
        thread::spawn(move || forward_output(Buffer::Stdout, stdout_rx));

        let (stderr_tx, stderr_rx) = mpsc::sync_channel(1);
        thread::spawn(move || forward_output(Buffer::Stderr, stderr_rx));

        Self {
            stdout_tx,
            stderr_tx,
        }
    }

    fn run(&self, command: &mut Command) -> Running {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let (output_tx, output_rx) = mpsc::sync_channel(1);

        self.stdout_tx.send((stdout, output_tx.clone())).ok();
        self.stderr_tx.send((stderr, output_tx)).ok();

        Running {
            child,
            output_rx,
            start: Instant::now(),
            stdout: Vec::new(),
            stdout_offset: 0,
            stderr: Vec::new(),
            stderr_offset: 0,
        }
    }
}

struct Running {
    child: Child,
    output_rx: mpsc::Receiver<Chunk>,
    start: Instant,
    stdout: Vec<u8>,
    stdout_offset: usize,
    stderr: Vec<u8>,
    stderr_offset: usize,
}

impl Running {
    fn next(&mut self, timeout: Duration) -> Status {
        loop {
            match self.output_rx.recv_timeout(timeout) {
                Ok((buffer, mut chunk)) => match buffer {
                    Buffer::Stdout => {
                        self.stdout.append(&mut chunk);
                    }
                    Buffer::Stderr => {
                        self.stderr.append(&mut chunk);
                    }
                },
                Err(RecvTimeoutError::Timeout) => {
                    let stdout = self.stdout[self.stdout_offset..].to_vec();
                    self.stdout_offset = self.stdout.len();

                    let stderr = self.stderr[self.stderr_offset..].to_vec();
                    self.stderr_offset = self.stderr.len();

                    break Status::Slow {
                        elapsed: self.start.elapsed(),
                        stdout,
                        stderr,
                    };
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let status = self.child.wait().unwrap();

                    if status.success() {
                        break Status::Success;
                    } else {
                        break Status::Failure {
                            stdout: mem::take(&mut self.stdout),
                            stderr: mem::take(&mut self.stderr),
                            code: status.code(),
                        };
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Buffer {
    Stdout,
    Stderr,
}

type Chunk = (Buffer, Vec<u8>);

enum Status {
    Success,
    Failure {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        code: Option<i32>,
    },
    Slow {
        elapsed: Duration,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
}
