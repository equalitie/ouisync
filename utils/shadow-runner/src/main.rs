//! Run command using the [shadow](https://github.com/shadow/shadow) simulator. Should be used as
//! [cargo runner](https://doc.rust-lang.org/cargo/reference/config.html#targettriplerunner).
//!
//! # Example
//!
//! ```bash
//! CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER=shadow-runner cargo test -p my-package
//! ```

use std::{
    env,
    fs::{self, File},
    io::{self, PipeWriter, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Result, bail, format_err};
use rand::{Rng, rngs::OsRng};
use serde_json::{Map, json};
use tempfile::TempDir;

const HOST_NAME: &str = "main";

fn main() -> Result<()> {
    let Some(command) = env::args().nth(1) else {
        println!(
            "Usage: {} <COMMAND> [ARGS]...",
            env::current_exe()
                .ok()
                .as_deref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("???")
        );
        bail!("Missing command");
    };

    let args: Vec<_> = env::args().skip(2).collect();

    let seed: u32 = if let Ok(seed) = env::var("SHADOW_SEED") {
        seed.parse()?
    } else {
        OsRng.r#gen()
    };
    println!("SEED: {seed}");

    let (temp_dir, data_dir) = if let Ok(data_dir) = env::var("SHADOW_DATA") {
        let data_dir = PathBuf::from(data_dir);

        match fs::remove_dir_all(&data_dir) {
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }

        (None, data_dir)
    } else {
        let temp_dir = TempDir::new()?;
        let data_dir = temp_dir.path().join("shadow.data");

        (Some(temp_dir), data_dir)
    };

    let (config_reader, mut config_writer) = io::pipe()?;
    let child = Command::new("shadow")
        .arg("--seed")
        .arg(seed.to_string())
        .arg("--data-directory")
        .arg(&data_dir)
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(config_reader)
        .spawn()?;

    write_shadow_config(&mut config_writer, &command, &args)?;
    drop(config_writer);

    let printer = OutputPrinter::new(data_dir.join("hosts").join(HOST_NAME), command);
    let output = child.wait_with_output()?;
    drop(printer);

    if env::var("SHADOW_OUTPUT").is_ok() {
        print_shadow_output(&output)?;
    }

    if output.status.success() {
        Ok(())
    } else {
        if let Some(temp_dir) = temp_dir {
            let path = temp_dir.keep();
            println!("work directory persisted in {}", path.display());
        }

        if let Some(code) = output.status.code() {
            Err(format_err!("shadow terminated with exit code {code}"))
        } else {
            Err(format_err!("shadow terminated by signal"))
        }
    }
}

fn write_shadow_config(writer: &mut PipeWriter, command: &str, args: &[String]) -> Result<()> {
    let mut env = Map::new();

    if let Ok(value) = env::var("RUST_LOG") {
        env.insert("RUST_LOG".into(), value.into());
    }

    if let Ok(value) = env::var("RUST_BACKTRACE") {
        env.insert("RUST_BACKTRACE".into(), value.into());
    }

    // Explicitly set the number of rust test threads to prevent this warning in shadow:
    //
    //     Opening unsupported proc file. Contents may incorrectly refer to native process instead
    //     of emulated, and/or have nondeterministic contents: /proc/self/cgroup
    //
    // Note this makes the tests run sequentially. Parallellism can be still achieved by running
    // each test in a separate process (for example, using `cargo nextest` or
    // `ouisync-stress-test`).
    env.insert("RUST_TEST_THREADS".into(), "1".into());

    let stop_time = env::var("SHADOW_STOP_TIME");
    let stop_time = stop_time.as_deref().ok().unwrap_or("1m");

    let config = json!({
        "general": {
            "stop_time": stop_time,
            "model_unblocked_syscall_latency": true,
        },
        "experimental": {
            // This config param including its comment taken from shadow-exec:
            //
            // For the sort of small simulations this tool is meant for, cpu
            // pinning is probably more trouble than its worth. e.g. multiple
            // simulations run at once will pin to the same cpu.
            "use_cpu_pinning": false,

            // https://github.com/shadow/shadow/discussions/3729#discussioncomment-15938874
            "max_unapplied_cpu_latency": "1ms",
        },
        "network": {
            "graph": {
                "type": "1_gbit_switch"
            }
        },
        "hosts": {
            HOST_NAME: {
                "network_node_id": 0,
                "processes": [
                    {
                        "path": command,
                        "args": args,
                        "environment": env,
                        "expected_final_state": {
                            "exited": 0
                        }
                    }
                ],
            }
        }
    });

    serde_json::to_writer(writer, &config)?;

    Ok(())
}

fn print_shadow_output(output: &Output) -> Result<()> {
    const DIVIDER: &str = "-----------------------------------------------------------------";

    let mut w = io::stderr();

    writeln!(&mut w, "{DIVIDER}")?;
    writeln!(&mut w, "shadow stdout:")?;
    writeln!(&mut w)?;
    w.write_all(&output.stdout)?;

    writeln!(&mut w)?;

    writeln!(&mut w, "{DIVIDER}")?;
    writeln!(&mut w, "shadow stderr:")?;
    writeln!(&mut w)?;
    w.write_all(&output.stderr)?;
    writeln!(&mut w)?;
    writeln!(&mut w, "{DIVIDER}")?;

    Ok(())
}

struct OutputPrinter {
    running: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl OutputPrinter {
    fn new(dir: PathBuf, command: String) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let join_handle = thread::spawn({
            let running = running.clone();
            move || run(running, &dir, &command)
        });

        Self {
            running,
            join_handle: Some(join_handle),
        }
    }
}

impl Drop for OutputPrinter {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);

        if let Some(join_handle) = self.join_handle.take() {
            join_handle.join().ok();
        }
    }
}

fn run(running: Arc<AtomicBool>, dir: &Path, command: &str) {
    let command = Path::new(command).file_name().unwrap();
    let stdout_path = dir.join(command).with_extension("1000.stdout");
    let stderr_path = dir.join(command).with_extension("1000.stderr");

    // Tail the stdout file
    let mut pos = 0;
    let mut stdout = io::stdout();

    while running.load(Ordering::Acquire) {
        match copy_file(&stdout_path, pos, &mut stdout) {
            Ok(n @ 1..) => pos += n,
            Ok(0) | Err(_) => {
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        }
    }

    // Print rest of the stdout file
    copy_file(&stdout_path, pos, &mut stdout).ok();
    // Print the stderr file
    copy_file(&stderr_path, 0, &mut io::stderr()).ok();
}

fn copy_file(path: &Path, offset: u64, dst: &mut impl io::Write) -> io::Result<u64> {
    let mut file = File::open(path)?;

    if offset > 0 {
        file.seek(SeekFrom::Start(offset))?;
    }

    io::copy(&mut file, dst)
}
