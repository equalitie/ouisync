mod client;
mod defaults;
mod format;
mod options;
mod server;

use clap::Parser;
use options::{Command, Options};
use ouisync_service::{transport::ClientError, Error as ServerError};
use std::{error::Error as _, process::ExitCode};
use thiserror::Error;

#[tokio::main]
async fn main() -> ExitCode {
    let options = Options::parse();

    let result = match options.command {
        Command::Server(command) => server::run(options.config_dir, command)
            .await
            .map_err(Error::from),
        Command::Client(command) => client::run(options.config_dir, command)
            .await
            .map_err(Error::from),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let mut source = error.source();
            let mut first = true;

            while let Some(error) = source {
                eprint!("{}{}", if first { "" } else { " → " }, error);
                source = error.source();
                first = false;
            }

            eprintln!();

            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Error)]
enum Error {
    #[error("server error")]
    Server(#[from] ServerError),
    #[error("client error")]
    Client(#[from] ClientError),
}
