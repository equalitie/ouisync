mod client;
mod error;
mod geo_ip;
mod handler;
mod metrics;
mod options;
mod repository;
mod server;
mod state;
mod utils;

use clap::Parser;
use options::{Command, Options};
use std::process::ExitCode;

pub(crate) const APP_NAME: &str = "ouisync";
pub(crate) const DB_EXTENSION: &str = "ouisyncdb";

#[tokio::main]
async fn main() -> ExitCode {
    let options = Options::parse();

    let result = match options.command {
        Command::Start {
            config_dir,
            log_format,
            log_color,
        } => server::run(options.socket, config_dir, log_format, log_color).await,
        Command::Request(request) => client::run(options.socket, request).await,
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{:#}", error);
            ExitCode::FAILURE
        }
    }
}
