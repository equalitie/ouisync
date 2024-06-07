use anyhow::{format_err, Result};
use clap::Parser;
use comfy_table::{Attribute, Cell, CellAlignment, Table};
use indicatif::HumanBytes;
use rand::seq::SliceRandom;
use serde::{de::Error as _, Deserialize, Deserializer};
use std::{
    env,
    ffi::OsString,
    fs::{self, File},
    io::{BufRead, BufReader},
    ops::{Add, Div},
    path::{Path, PathBuf},
    process::{self, Stdio},
    str,
    time::Duration,
};
use tempfile::NamedTempFile;

const BENCH_DIR: &str = "benches";

fn main() -> Result<()> {
    let options = Options::parse();

    build(&options)?;
    run(&options)?;

    Ok(())
}

/// Build, run and compare different versions of the same benchmark.
///
/// Typical usage is to build a bench in the master branch, then switch to a branch that contains
/// potential perf improvements, build the same bench there, run both versions and compare the
/// results:
///
///     > git checkout master
///     > cargo run -p benchtool -- <BENCH_NAME> --build
///     > git checkout perf-improvements
///     > cargo run -p benchtool -- <BENCH_NAME> --run --samples 10
#[derive(Parser, Debug)]
#[command(verbatim_doc_comment)]
struct Options {
    /// Package to build
    #[arg(short, long, value_name = "SPEC")]
    package: Option<String>,

    /// Directory for all generated artifacts
    #[arg(long, value_name = "PATH")]
    target_dir: Option<PathBuf>,

    /// Space or comma separated list of features to activate
    #[arg(short = 'F', long)]
    features: Vec<String>,

    /// Coloring: auto, always, never
    #[arg(long, default_value = "auto")]
    color: String,

    /// Build the bench. The resulting bench binary is labeled with LABEL if specified or the
    /// current git branch name if not.
    #[arg(short, long, value_name = "LABEL")]
    build: Option<Option<String>>,

    /// Run the bench. If LABEL is specified runs only the bench version with that label, otherwise
    /// run all versions in random order.
    #[arg(short, long, value_delimiter = ',' , num_args = 0.., value_name = "LABEL")]
    run: Option<Vec<String>>,

    /// Run each bench version this many times and average the results.
    #[arg(short, long, default_value_t = 1)]
    samples: usize,

    /// Bench target to build and/or run.
    #[arg(value_name = "NAME")]
    bench: String,

    /// Args to the bench target.
    #[arg(trailing_var_arg = true)]
    args: Vec<OsString>,
}

impl Options {
    fn bench_dir(&self) -> PathBuf {
        self.target_dir
            .as_ref()
            .cloned()
            .unwrap_or_else(default_target_dir)
            .join(BENCH_DIR)
    }
}

fn build(options: &Options) -> Result<()> {
    let Some(label) = &options.build else {
        return Ok(());
    };

    let mut command = process::Command::new("cargo");
    command
        .arg("build")
        .arg("--message-format")
        .arg("json")
        .arg("--color")
        .arg(&options.color);

    if let Some(value) = &options.target_dir {
        command.arg("--target-dir").arg(value);
    }

    if let Some(value) = &options.package {
        command.arg("--package").arg(value);
    }

    for value in &options.features {
        command.arg("--features").arg(value);
    }

    command.arg("--bench").arg(&options.bench);

    println!(
        "Running `{} {}`",
        format_command(&command),
        format_command_args(&command)
    );

    command.stderr(Stdio::inherit());

    let output = command.output()?;

    if !output.status.success() {
        return Err(format_err!(
            "{} returned {}",
            format_command(&command),
            output.status
        ));
    }

    let stdout = BufReader::new(&output.stdout[..]);
    let dst_dir = options.bench_dir();

    let label = match label {
        Some(label) => label.to_owned(),
        None => get_default_label()?,
    };

    for line in stdout.lines() {
        let line = line?;
        let message: BuildMessage = serde_json::from_str(&line)?;

        let Some(src) = message.executable else {
            continue;
        };

        let dst = make_bench_file_name(src, &label);
        let dst = dst_dir.join(dst);

        println!("Built bench: {}", dst.display());

        fs::create_dir_all(dst.parent().unwrap())?;
        fs::copy(src, dst)?;
    }

    Ok(())
}

fn run(options: &Options) -> Result<()> {
    let Some(label) = &options.run else {
        return Ok(());
    };

    let dir = options.bench_dir();
    let mut bench_versions = list_bench_versions(&dir, &options.bench, label.as_ref())?;
    let mut output = NamedTempFile::new()?;
    let mut rng = rand::thread_rng();

    for i in 0..options.samples {
        println!("Running sample {}/{}", i + 1, options.samples);
        println!();

        bench_versions.shuffle(&mut rng);

        for bench_version in &bench_versions {
            let label = extract_bench_label(bench_version);

            let mut command = process::Command::new(bench_version);
            command
                .arg("--label")
                .arg(label)
                .arg("--output")
                .arg(output.path());

            for arg in &options.args {
                command.arg(arg);
            }

            println!(
                "🚀🚀🚀 Running `{} {}` 🚀🚀🚀",
                format_command(&command),
                format_command_args(&command)
            );

            let status = command.status()?;
            if !status.success() {
                return Err(format_err!(
                    "{} returned {}",
                    format_command(&command),
                    status
                ));
            }
        }

        println!();
    }

    let summaries = read_summaries(output.as_file_mut())?;
    let summaries = aggregate_summaries(summaries);

    let table = build_comparison_table(summaries);

    println!("{table}");

    Ok(())
}

fn list_bench_versions(dir: &Path, bench: &str, labels: &[String]) -> Result<Vec<PathBuf>> {
    let suffixes: Vec<_> = labels.iter().map(|label| format!("@{label}")).collect();

    fs::read_dir(dir)?
        .map(|entry| {
            let path = entry?.path();
            let name = match path
                .with_extension("")
                .file_name()
                .and_then(|name| name.to_str())
            {
                Some(name) => name.to_owned(),
                None => return Ok(None),
            };

            if !name.starts_with(bench) {
                return Ok(None);
            }

            if !suffixes.is_empty() && !suffixes.iter().any(|suffix| name.ends_with(suffix)) {
                return Ok(None);
            }

            Ok(Some(path))
        })
        .filter_map(Result::transpose)
        .collect()
}

fn default_target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target"))
}

fn make_bench_file_name(src: &Path, label: &str) -> PathBuf {
    let name = src.file_stem().unwrap();
    let ext = src.extension();

    let name = name.to_str().unwrap();
    let name = name
        .rsplit_once('-')
        .map(|(prefix, _)| prefix)
        .unwrap_or(name);

    let mut name = PathBuf::from(format!("{name}@{label}"));

    if let Some(ext) = ext {
        name.set_extension(ext);
    }

    name
}

fn extract_bench_label(path: &Path) -> &str {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.rsplit_once('@'))
        .map(|(_, suffix)| suffix)
        .unwrap_or_default()
}

fn get_git_commit() -> Result<String> {
    Ok(str::from_utf8(
        process::Command::new("git")
            .arg("rev-parse")
            .arg("--short")
            .arg("HEAD")
            .output()?
            .stdout
            .as_ref(),
    )?
    .trim()
    .to_owned())
}

fn get_git_branch() -> Result<String> {
    Ok(str::from_utf8(
        process::Command::new("git")
            .arg("rev-parse")
            .arg("--abbrev-ref")
            .arg("HEAD")
            .output()?
            .stdout
            .as_ref(),
    )?
    .trim()
    .to_owned())
}

fn get_default_label() -> Result<String> {
    let branch = get_git_branch()?;

    if branch != "HEAD" {
        Ok(branch)
    } else {
        get_git_commit()
    }
}

fn format_command(command: &process::Command) -> String {
    command.get_program().to_str().unwrap().to_owned()
}

fn format_command_args(command: &process::Command) -> String {
    command
        .get_args()
        .map(|arg| arg.to_str().unwrap())
        .collect::<Vec<_>>()
        .join(" ")
}

fn deserialize_duration<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
    let secs = f64::deserialize(d)?;
    Duration::try_from_secs_f64(secs).map_err(D::Error::custom)
}

fn read_summaries(file: &mut File) -> Result<Vec<Summary>> {
    BufReader::new(file)
        .lines()
        .map(|line| Ok(serde_json::from_slice(line?.as_bytes())?))
        .collect()
}

fn aggregate_summaries(mut summaries: Vec<Summary>) -> Vec<Summary> {
    summaries.sort_by(|a, b| a.label.cmp(&b.label));
    summaries
        .chunk_by(|a, b| a.label == b.label)
        .map(Summary::avg)
        .collect()
}

fn build_comparison_table(summaries: Vec<Summary>) -> Table {
    let mut table = Table::new();

    table.set_header(vec![
        Cell::new("label"),
        Cell::new("duration"),
        Cell::new("send min").add_attribute(Attribute::Dim),
        Cell::new("send max").add_attribute(Attribute::Dim),
        Cell::new("send mean").add_attribute(Attribute::Dim),
        Cell::new("send stdev"),
        Cell::new("recv min").add_attribute(Attribute::Dim),
        Cell::new("recv max").add_attribute(Attribute::Dim),
        Cell::new("recv mean").add_attribute(Attribute::Dim),
        Cell::new("recv stdev"),
    ]);

    for summary in summaries {
        table.add_row(vec![
            Cell::new(summary.label),
            Cell::new(format!("{:.2} s", summary.duration.as_secs_f64())),
            Cell::new(HumanBytes(summary.send.min))
                .set_alignment(CellAlignment::Right)
                .add_attribute(Attribute::Dim),
            Cell::new(HumanBytes(summary.send.max))
                .set_alignment(CellAlignment::Right)
                .add_attribute(Attribute::Dim),
            Cell::new(HumanBytes(summary.send.mean))
                .set_alignment(CellAlignment::Right)
                .add_attribute(Attribute::Dim),
            Cell::new(HumanBytes(summary.send.stdev)).set_alignment(CellAlignment::Right),
            Cell::new(HumanBytes(summary.recv.min))
                .set_alignment(CellAlignment::Right)
                .add_attribute(Attribute::Dim),
            Cell::new(HumanBytes(summary.recv.max))
                .set_alignment(CellAlignment::Right)
                .add_attribute(Attribute::Dim),
            Cell::new(HumanBytes(summary.recv.mean))
                .set_alignment(CellAlignment::Right)
                .add_attribute(Attribute::Dim),
            Cell::new(HumanBytes(summary.recv.stdev)).set_alignment(CellAlignment::Right),
        ]);
    }

    table
}

#[derive(Deserialize)]
struct BuildMessage<'a> {
    #[serde(borrow)]
    executable: Option<&'a Path>,
}

#[derive(Default, Debug, Deserialize)]
struct Summary {
    #[serde(default)]
    label: String,
    #[serde(deserialize_with = "deserialize_duration")]
    duration: Duration,
    send: BytesSummary,
    recv: BytesSummary,
}

impl Summary {
    fn avg<'a>(iter: impl IntoIterator<Item = &'a Summary>) -> Self {
        let (sum, count) =
            iter.into_iter()
                .fold((Summary::default(), 0u32), |(sum, count), item| {
                    (
                        Summary {
                            label: if sum.label.is_empty() {
                                item.label.clone()
                            } else {
                                sum.label
                            },
                            duration: sum.duration + item.duration,
                            send: sum.send + item.send,
                            recv: sum.recv + item.recv,
                        },
                        count + 1,
                    )
                });

        sum / count
    }
}

impl Div<u32> for Summary {
    type Output = Self;

    fn div(self, rhs: u32) -> Self::Output {
        Self {
            label: self.label,
            duration: self.duration / rhs,
            send: self.send / rhs,
            recv: self.recv / rhs,
        }
    }
}

#[derive(Default, Copy, Clone, Debug, Deserialize)]
struct BytesSummary {
    min: u64,
    max: u64,
    mean: u64,
    stdev: u64,
}

impl Add for BytesSummary {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            min: self.min + rhs.min,
            max: self.max + rhs.max,
            mean: self.mean + rhs.mean,
            stdev: self.stdev + rhs.stdev,
        }
    }
}

impl Div<u32> for BytesSummary {
    type Output = Self;

    fn div(self, rhs: u32) -> Self::Output {
        Self {
            min: self.min / rhs as u64,
            max: self.max / rhs as u64,
            mean: self.mean / rhs as u64,
            stdev: self.stdev / rhs as u64,
        }
    }
}
