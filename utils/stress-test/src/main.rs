use chrono::{DateTime, Local};
use clap::{value_parser, Parser};
use serde::Deserialize;
use std::{
    env,
    io::{self, BufRead, BufReader},
    process::{self, Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::SystemTime,
};
use tempfile::TempDir;

fn main() {
    let options = Options::parse();

    // Override TMPDIR so that all temp files/directories created in the tests are created inside
    // it, for easier cleanup.
    let temp_root = TempDir::new().unwrap();
    env::set_var("TMPDIR", temp_root.path()); // unix
    env::set_var("TEMP", temp_root.path()); // windows

    // Build the test binaries
    let exes = build(&options);

    let args: Vec<_> = options
        .exact
        .then_some("--exact")
        .into_iter()
        .chain(options.skip.iter().flat_map(|filter| ["--skip", filter]))
        .chain(options.filters.iter().map(|filter| filter.as_str()))
        .map(|s| s.to_owned())
        .collect();

    let args_display = args.join(" ");

    println!(
        "Starting {} {}:",
        options.concurrency,
        if options.concurrency == 1 {
            "process"
        } else {
            "processes"
        }
    );

    for exe in &exes {
        println!("    {exe} {args_display}");
    }

    let (tx, rx) = mpsc::sync_channel(0);

    for index in 0..options.concurrency {
        thread::spawn({
            let exes = exes.clone();
            let args = args.clone();
            let tx = tx.clone();
            move || run(index, exes, args, tx)
        });
    }

    for (global_iteration, status) in rx.into_iter().enumerate() {
        let timestamp: DateTime<Local> = SystemTime::now().into();

        print!(
            "{} Iteration #{global_iteration}\t({}/{})",
            timestamp.format("%Y-%m-%d %H:%M:%S"),
            status.process,
            status.iteration
        );

        match status.result {
            Ok(()) => println!(),
            Err(output) => {
                println!("\tFAILURE ({})", output.status);

                println!("\n\n---- stdout: ----\n\n");
                io::copy(&mut &output.stdout[..], &mut io::stdout()).unwrap();

                println!("\n\n---- stderr: ----\n\n");
                io::copy(&mut &output.stderr[..], &mut io::stdout()).unwrap();

                break;
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

    /// Test only this package's library
    #[arg(long)]
    lib: bool,

    /// Test only the specified test target
    #[arg(long, value_name = "NAME")]
    test: Vec<String>,

    /// Skip tests whose names contain FILTER
    #[arg(long, value_name = "FILTER")]
    skip: Vec<String>,

    /// Exactly match filters rather than by substring
    #[arg(long)]
    exact: bool,

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
        .arg("--release")
        .arg("--message-format")
        .arg("json");

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
        .filter_map(|line| {
            let line = line.unwrap();
            let message: BuildMessage = serde_json::from_str(&line).unwrap();

            message.executable.map(|exe| exe.to_owned())
        })
        .collect()
}

#[derive(Deserialize)]
struct BuildMessage<'a> {
    executable: Option<&'a str>,
}

fn run(process: u64, exes: Vec<String>, args: Vec<String>, tx: mpsc::SyncSender<Status>) {
    let mut commands: Vec<_> = exes
        .into_iter()
        .map(|exe| {
            let mut command = Command::new(exe);
            command.args(&args);
            command
        })
        .collect();

    for iteration in 0.. {
        let result = commands
            .iter_mut()
            .map(|command| command.output().unwrap())
            .find(|output| !output.status.success())
            .map(Err)
            .unwrap_or(Ok(()));
        let is_failure = result.is_err();

        let status = Status {
            process,
            iteration,
            result,
        };

        if tx.send(status).is_err() {
            break;
        }

        if is_failure {
            break;
        }
    }
}

struct Status {
    process: u64,
    iteration: u64,
    result: Result<(), Output>,
}
