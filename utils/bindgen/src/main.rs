mod dart;
mod kotlin;
mod parse;

use clap::{Parser, ValueEnum};
use parse::{parse_file, Source};
use std::io;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse();

    let source_files = [
        "service/src/protocol.rs",
        "lib/src/access_control/access_mode.rs",
        "lib/src/directory/entry_type.rs",
        "lib/src/network/peer_source.rs",
        "lib/src/network/peer_state.rs",
    ];
    let mut source = Source::new();

    for source_file in source_files {
        parse_file(source_file, &mut source)?;
    }

    match options.language {
        Language::Dart => dart::generate(&source, &mut io::stdout())?,
        Language::Kotlin => kotlin::generate(&source, &mut io::stdout())?,
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
