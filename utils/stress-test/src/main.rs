use clap::{Parser, builder::TypedValueParser, value_parser};
use serde::Deserialize;
use std::{
    borrow::Cow,
    env,
    ffi::OsStr,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Seek, SeekFrom},
    path::PathBuf,
    process::{self, Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        mpsc::{self, RecvTimeoutError},
    },
    thread::{self},
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn main() {
    let options = Options::parse();

    let temp_root = TempDir::new().unwrap();

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
    let handles: Vec<_> = (0..options.concurrency)
        .map(|index| {
            thread::spawn({
                let runner = runner.clone();
                let exes = exes.clone();
                let args = args.clone();
                let slow = options.slow;
                let temp_root = temp_root.path().to_owned();
                let tx = tx.clone();
                move || run(index, runner, exes, args, slow, temp_root, tx)
            })
        })
        .collect();

    ctrlc::set_handler(move || {
        tx.send(Event::Terminate).ok();
    })
    .unwrap();

    let start = Instant::now();

    for (global_iteration, event) in rx.into_iter().enumerate() {
        let progress = match event {
            Event::Progress(progress) => progress,
            Event::Terminate => break,
        };

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
                result,
            } => {
                print!("\tFAILURE (");
                match result {
                    Ok(Some(code)) => print!("exit code: {code}"),
                    Ok(None) => print!("terminated by signal"),
                    Err(error) => print!("failed to get exit code: {error:?}"),
                }
                println!(")");

                println!("\n\n---- stdout: ----\n\n");
                stdout.as_ref().seek(SeekFrom::Start(0)).unwrap();
                io::copy(&mut stdout.as_ref(), &mut io::stdout()).unwrap();

                println!("\n\n---- stderr: ----\n\n");
                stderr.as_ref().seek(SeekFrom::Start(0)).unwrap();
                io::copy(&mut stderr.as_ref(), &mut io::stdout()).unwrap();

                let path = temp_root.keep();
                println!("\n\nwork directory persisted in {}", path.display());

                break;
            }
            Status::Slow {
                command,
                elapsed,
                stdout,
                stderr,
            } => {
                println!("\ttaking too long (so far: {})", DisplayDuration(elapsed));
                println!();
                println!("command: '{command}'");
                println!("output since last report:");

                println!("\n\n---- stdout: ----\n\n");
                io::copy(&mut stdout.as_ref(), &mut io::stdout()).unwrap();

                println!("\n\n---- stderr: ----\n\n");
                io::copy(&mut stderr.as_ref(), &mut io::stdout()).unwrap();

                println!("\n\n");
            }
        }
    }

    for handle in handles {
        handle.join().ok();
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

    /// Report tests that take longer than this. The value is interpreted as seconds unless a unit
    /// suffix is specified. Allowed unit suffixes: h, m, s, ms. Example: "10m"
    #[arg(long, value_name = "DURATION", default_value = "1m", value_parser = DurationParser)]
    slow: Duration,

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

    println!("Running `{}`", DisplayCommand(&command));

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
    slow: Duration,
    temp_root: PathBuf,
    tx: mpsc::SyncSender<Event>,
) {
    let mut commands = make_commands(runner, exes, args);

    let runner = CommandRunner::new(temp_root);

    for iteration in 0.. {
        for command in &mut commands {
            let mut running = runner.run(command);

            loop {
                match running.next(slow) {
                    Status::Success => break,
                    status @ Status::Failure { .. } => {
                        tx.send(Event::Progress(Progress {
                            process,
                            iteration,
                            status,
                        }))
                        .ok();
                        return;
                    }
                    status @ Status::Slow { .. } => {
                        if tx
                            .send(Event::Progress(Progress {
                                process,
                                iteration,
                                status,
                            }))
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
        }

        if tx
            .send(Event::Progress(Progress {
                process,
                iteration,
                status: Status::Success,
            }))
            .is_err()
        {
            break;
        }
    }
}

fn make_commands(runner: Option<String>, exes: Vec<String>, args: Vec<String>) -> Vec<Command> {
    if let Some(runner) = runner {
        // If runner is specified, run each test in a separate command. To find what tests to run,
        // run the executable with `--list` first. This assumes the executables are regular rust
        // test executables.
        //
        // This is useful to prevent tests from affecting the runner state of other tests. For
        // example, when running the tests with the shadow simulator this allows reproducing
        // failing tests in isolation.
        let mut commands = Vec::new();

        let mut test_args = Vec::new();
        let mut args_iter = args.iter().map(|s| s.as_str());

        loop {
            let Some(arg) = args_iter.next() else {
                break;
            };

            // collect args without values
            if arg == "--no-capture"
                || arg == "--nocapture"
                || arg == "--show-output"
                || arg == "--quiet"
                || arg == "-q"
            {
                test_args.push(arg);
                continue;
            }

            // collect args with values in the "--name value" form
            if arg == "--test-threads" || arg == "--color" || arg == "--format" {
                test_args.push(arg);
                test_args.extend(args_iter.next());
                continue;
            }

            // collect args with values in the "--name=value" form
            if let Some((name, value)) = arg.split_once("=")
                && (name == "--test-threads" || arg == "--color" || arg == "--format")
            {
                test_args.push(name);
                test_args.push(value);
            }
        }

        for exe in exes {
            let output = match Command::new(&exe).arg("--list").args(&args).output() {
                Ok(output) => output,
                Err(error) => panic!("'{exe} --list' failed to run: {error:?}"),
            };

            if !output.status.success() {
                if let Some(code) = output.status.code() {
                    panic!("'{exe} --list' exited with exit code {code}");
                } else {
                    panic!("'{exe} --list' terminated by signal");
                }
            }

            let content = match String::from_utf8(output.stdout) {
                Ok(content) => content,
                Err(error) => {
                    panic!("'{exe} --list' invalid output: {error:?}")
                }
            };

            let mut count = 0;

            for line in content.lines() {
                let Some(name) = line.trim().strip_suffix(": test") else {
                    continue;
                };

                commands.push({
                    let mut command = Command::new(&runner);
                    command.arg(&exe).arg("--exact").arg(name).args(&test_args);
                    command
                });

                count += 1;
            }

            if count == 0 {
                panic!("'{exe} {}' no tests matched", args.join(" "))
            }
        }

        commands
    } else {
        exes.into_iter()
            .map(|exe| {
                let mut command = Command::new(exe);
                command.args(&args);
                command
            })
            .collect()
    }
}

enum Event {
    Progress(Progress),
    Terminate,
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

struct DisplayCommand<'a>(&'a Command);

impl fmt::Display for DisplayCommand<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.get_program().to_string_lossy())?;

        for arg in self.0.get_args() {
            write!(f, " {}", arg.to_string_lossy())?;
        }

        Ok(())
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

type CommandRequest = (Child, mpsc::SyncSender<io::Result<ExitStatus>>);

struct CommandRunner {
    request_tx: mpsc::SyncSender<CommandRequest>,
    temp_dir: PathBuf,
}

impl CommandRunner {
    fn new(temp_dir: PathBuf) -> Self {
        let (request_tx, request_rx) = mpsc::sync_channel::<CommandRequest>(1);

        thread::spawn(move || {
            for (mut child, response_tx) in request_rx {
                response_tx.send(child.wait()).ok();
            }
        });

        Self {
            request_tx,
            temp_dir,
        }
    }

    fn run<'a>(&'_ self, command: &'a mut Command) -> Running<'a> {
        let (stdout_writer, stdout_reader) = self.new_buffer("stdout");
        let (stderr_writer, stderr_reader) = self.new_buffer("stderr");

        let temp_dir = TempDir::new_in(&self.temp_dir).unwrap();
        let temp_dir_env_name = if cfg!(windows) { "TEMP" } else { "TMPDIR" };

        let child = command
            .stdout(stdout_writer)
            .stderr(stderr_writer)
            .env(temp_dir_env_name, temp_dir.path())
            .spawn()
            .unwrap();

        let (response_tx, response_rx) = mpsc::sync_channel(1);

        self.request_tx.send((child, response_tx)).unwrap();

        Running {
            command,
            response_rx,
            start: Instant::now(),
            stdout: Arc::new(stdout_reader),
            stderr: Arc::new(stderr_reader),
            _temp_dir: temp_dir,
        }
    }

    fn new_buffer(&self, name: &str) -> (File, File) {
        let path = self.temp_dir.join(name);

        match fs::remove_file(&path) {
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => panic!("failed to remove file at '{}': {:?}", path.display(), error),
        }

        let writer = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap_or_else(|error| {
                panic!(
                    "failed to open file '{}' for writing: {error:?}",
                    path.display()
                )
            });

        let reader = OpenOptions::new()
            .read(true)
            .open(&path)
            .unwrap_or_else(|error| {
                panic!(
                    "failed to open file '{}' for reading: {error:?}",
                    path.display()
                )
            });

        (writer, reader)
    }
}

struct Running<'a> {
    command: &'a Command,
    response_rx: mpsc::Receiver<io::Result<ExitStatus>>,
    start: Instant,
    stdout: Arc<File>,
    stderr: Arc<File>,
    _temp_dir: TempDir,
}

impl Running<'_> {
    fn next(&mut self, timeout: Duration) -> Status {
        match self.response_rx.recv_timeout(timeout) {
            Ok(result) => match result {
                Ok(status) if status.success() => Status::Success,
                result => Status::Failure {
                    stdout: self.stdout.clone(),
                    stderr: self.stderr.clone(),
                    result: result.map(|status| status.code()),
                },
            },
            Err(RecvTimeoutError::Timeout) => Status::Slow {
                command: DisplayCommand(self.command).to_string(),
                elapsed: self.start.elapsed(),
                stdout: self.stdout.clone(),
                stderr: self.stderr.clone(),
            },
            Err(RecvTimeoutError::Disconnected) => unreachable!(),
        }
    }
}

enum Status {
    Success,
    Failure {
        stdout: Arc<File>,
        stderr: Arc<File>,
        result: io::Result<Option<i32>>,
    },
    Slow {
        command: String,
        elapsed: Duration,
        stdout: Arc<File>,
        stderr: Arc<File>,
    },
}

#[derive(Clone)]
struct DurationParser;

impl TypedValueParser for DurationParser {
    type Value = Duration;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        use clap::error::{ContextKind, ContextValue, ErrorKind};

        let make_error = |kind: ErrorKind| {
            let mut error = clap::Error::new(kind).with_cmd(cmd);

            if let Some(arg) = arg {
                error.insert(
                    ContextKind::InvalidArg,
                    ContextValue::String(arg.to_string()),
                );
            }

            if let Some(value) = value.to_str() {
                error.insert(
                    ContextKind::InvalidValue,
                    ContextValue::String(value.to_string()),
                );
            }

            error
        };

        let parse_value = |input: &str| {
            input
                .trim()
                .parse()
                .map_err(|_| make_error(ErrorKind::InvalidValue))
        };

        let value = value
            .to_str()
            .ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8).with_cmd(cmd))?
            .trim();

        for (suffix, map) in [
            ("ms", Duration::from_millis as fn(u64) -> Duration),
            ("s", Duration::from_secs),
            ("m", Duration::from_mins),
            ("h", Duration::from_hours),
        ] {
            if let Some(value) = value.strip_suffix(suffix) {
                return Ok(map(parse_value(value)?));
            }
        }

        Ok(Duration::from_secs(parse_value(value)?))
    }
}
