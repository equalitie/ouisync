use ouisync::{AccessMode, AccessSecrets, ShareToken};
use std::env;

fn main() {
    let arg = env::args().nth(1).unwrap_or_default();

    match arg.as_str() {
        "-w" | "--write" => run(AccessMode::Write, env::args().nth(2).as_deref()),
        "-r" | "--read" => run(AccessMode::Read, env::args().nth(2).as_deref()),
        "-b" | "--blind" => run(AccessMode::Blind, env::args().nth(2).as_deref()),
        "-v" | "--version" => {
            println!("{}", env!("CARGO_PKG_VERSION"));
        }
        "-i" | "--info" => {
            let Some(token) = env::args().nth(2) else {
                println!("Missing [TOKEN] argument");
                println!();
                help();
                return;
            };

            let Ok(token) = token.parse() else {
                println!("Failed to parse [TOKEN]");
                return;
            };

            show_token_information(&token);
        }
        "-h" | "--help" => {
            println!("Generate and convert Ouisync share tokens");
            println!();
            help();
        }
        o => {
            if o.is_empty() {
                println!("Missing option");
            } else {
                println!("Invalid option: {o}");
            }
            println!();
            help();
        }
    }
}

fn run(mode: AccessMode, input: Option<&str>) {
    let token = if let Some(token) = input {
        token.parse().unwrap()
    } else {
        ShareToken::from(AccessSecrets::random_write())
    };

    println!("{}", ShareToken::from(token.into_secrets().with_mode(mode)));
}

fn show_token_information(token: &ShareToken) {
    println!("AccessMode: {:?}", token.access_mode());
}

fn help() {
    println!("Usage: {} [OPTIONS]", env!("CARGO_PKG_NAME"));
    println!();
    println!("Options:");
    println!("  -w, --write          Generate new write token");
    println!("  -r, --read [TOKEN]   If TOKEN given, convert it to read token, otherwise generate new read token");
    println!("  -b, --blind [TOKEN]  If TOKEN given, convert it to blind token, otherwise generate new blind token");
    println!("  -i, --info [TOKEN]   Print information about the given token");
    println!("  -v, --version        Print version");
    println!("  -h, --help           Print help");
    println!();
}
