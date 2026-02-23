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
    fs::File,
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
        eprintln!(
            "Usage: {} COMMAND [ARGS...]",
            env::current_exe().unwrap().display()
        );
        bail!("Missing command");
    };
    let args: Vec<_> = env::args().skip(2).collect();

    let temp_dir = TempDir::new()?;
    let data_dir = temp_dir.path().join("shadow.data");

    let seed: u32 = if let Ok(seed) = env::var("SEED") {
        match seed.parse() {
            Ok(seed) => seed,
            Err(_) => bail!("Invalid SEED: '{}'", seed),
        }
    } else {
        OsRng.r#gen()
    };

    eprintln!("SEED: {seed}");

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

    if output.status.success() {
        Ok(())
    } else {
        print_shadow_output(&output)?;

        let path = temp_dir.keep();
        eprintln!("work directory persisted in {}", path.display());

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

    let config = json!({
        "general": {
            "stop_time": "10m"
        },
        "experimental": {
            // This config param including its comment taken from shadow-exec:
            //
            // For the sort of small simulations this tool is meant for, cpu
            // pinning is probably more trouble than its worth. e.g. multiple
            // simulations run at once will pin to the same cpu.
            "use_cpu_pinning": false,
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
    let mut w = io::stderr();

    writeln!(
        &mut w,
        "-----------------------------------------------------------------"
    )?;
    writeln!(&mut w, "shadow stdout:")?;
    writeln!(&mut w)?;
    w.write_all(&output.stdout)?;

    writeln!(&mut w)?;

    writeln!(
        &mut w,
        "-----------------------------------------------------------------"
    )?;
    writeln!(&mut w, "shadow stderr:")?;
    writeln!(&mut w)?;
    w.write_all(&output.stderr)?;

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
            Ok(n) => pos += n,
            Err(_) => {
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
