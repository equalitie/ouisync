mod dart;
mod kotlin;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use ouisync_api_parser::Context;
use std::io;

fn main() -> Result<()> {
    let options = Options::parse();

    let source_files = ["service/src/lib.rs", "lib/src/lib.rs"];
    let context = Context::parse(&source_files)?;

    match options.language {
        Language::Dart => dart::generate(&context, &mut io::stdout())?,
        Language::Kotlin => kotlin::generate(&context, &mut io::stdout())?,
    }

    Ok(())
}

#[derive(Parser, Debug)]
#[command(about)]
struct Options {
    /// Language to generate the bindings for
    #[arg(short, long)]
    language: Language,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Language {
    Dart,
    Kotlin,
}
