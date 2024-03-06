use ouisync::{AccessSecrets, ShareToken};
use std::env;

fn main() {
    let arg = env::args().nth(1).unwrap_or_default();

    match arg.as_str() {
        "-h" | "--help" => {
            println!("Generates random Ouisync repository share token with write access.");
            return;
        }
        "-v" | "--version" => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            return;
        }
        _ => (),
    }

    println!("{}", ShareToken::from(AccessSecrets::random_write()));
}
