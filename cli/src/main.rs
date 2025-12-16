mod client;
mod defaults;
mod format;
mod migration;
mod options;
mod server;

use clap::Parser;
use options::{Command, Options};
use ouisync_service::{Error as ServerError, protocol::ErrorCode, transport::ClientError};
use std::{env, fmt, process::ExitCode};
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
            eprintln!("{}", ErrorDisplay(error));
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Error)]
enum Error {
    #[error(transparent)]
    Server(#[from] ServerError),
    #[error(transparent)]
    Client(#[from] ClientError),
}

struct ErrorDisplay(Error);

impl fmt::Display for ErrorDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Error::Client(ClientError::InvalidEndpoint(_)) => {
                writeln!(
                    f,
                    "The server endpoint is invalid. This is probably caused by the server not \
                     running. Start it with `{0} start`. Run `{0} start --help` or `{0} --help` \
                     for more info.",
                    exe_name()
                )?;
                writeln!(f)?;
            }
            Error::Client(ClientError::Connect(_)) => {
                writeln!(
                    f,
                    "Failed to connect to the server. Ensure the server is running by invoking \
                    `{0} start`. Run `{0} start --help` or `{0} --help` for more info.",
                    exe_name()
                )?;
                writeln!(f)?;
            }
            Error::Client(ClientError::Response(error))
                if error.code() == ErrorCode::MountDirUnspecified =>
            {
                writeln!(
                    f,
                    "Mount directory not specified. Configure it by invoking `{0} mount-dir PATH`. \
                     Run `{0} mount-dir --help` or `{0} --help` for more info.",
                    exe_name()
                )?;
                writeln!(f)?;
            }
            // TODO: friendly message for more error types
            _ => (),
        }

        let mut next = Some(&self.0 as &dyn std::error::Error);
        let mut first = true;

        while let Some(error) = next {
            write!(f, "{}{}", if first { "" } else { " → " }, error)?;
            next = error.source();
            first = false;
        }

        Ok(())
    }
}

fn exe_name() -> String {
    env::current_exe()
        .ok()
        .as_deref()
        .and_then(|s| s.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("ouisync")
        .to_owned()
}
