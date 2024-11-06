mod client;
mod error;
mod geo_ip;
mod handler;
mod metrics;
mod options;
mod protocol;
mod repository;
mod server;
mod state;
mod transport;
mod utils;

use clap::Parser;
use options::Options;
use protocol::Request;
use std::process::ExitCode;

pub(crate) const APP_NAME: &str = "ouisync";
pub(crate) const DB_EXTENSION: &str = "ouisyncdb";

#[tokio::main]
async fn main() -> ExitCode {
    let options = Options::parse();

    let result = if let Request::Start = &options.request {
        server::run(
            options.dirs,
            options.socket,
            options.log_format,
            options.log_color,
        )
        .await
    } else {
        client::run(
            options.dirs,
            options.socket,
            options.log_format,
            options.log_color,
            options.request,
        )
        .await
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{:#}", error);
            ExitCode::FAILURE
        }
    }
}
