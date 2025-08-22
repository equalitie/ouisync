mod cpp;
mod dart;
mod kotlin;

use anyhow::Result;
use clap::{Parser, Subcommand};
use ouisync_api_parser::Context;
use std::{io, path::PathBuf};

fn main() -> Result<()> {
    let options = Options::parse();

    let source_files = ["lib/src/lib.rs", "net/src/lib.rs", "service/src/lib.rs"];
    let context = Context::parse(&source_files)?;

    match options.language {
        Language::Dart => dart::generate(&context, &mut io::stdout())?,
        Language::Kotlin => kotlin::generate(&context, &mut io::stdout())?,
        Language::Cpp { out_dir } => cpp::generate(&context, &out_dir)?,
    }

    Ok(())
}

#[derive(Parser, Debug)]
#[command(about)]
struct Options {
    /// Language to generate the bindings for
    #[command(subcommand)]
    language: Language,
}

#[derive(Clone, Debug, Subcommand)]
enum Language {
    Dart,
    Kotlin,
    Cpp {
        /// Output directory for generated *.{hpp,cpp} files
        out_dir: PathBuf,
    },
}
