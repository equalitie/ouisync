mod client;
mod error;
mod handler;
mod options;
mod repository;
mod server;
mod state;

use clap::Parser;
use client::ClientError;
use options::{Command, Options};
use ouisync_service::Error as ServerError;
use std::{fmt, process::ExitCode};

#[tokio::main]
async fn main() -> ExitCode {
    let options = Options::parse();

    let result = match options.command {
        Command::Server(command) => server::run(options.socket, command)
            .await
            .map_err(Error::from),
        Command::Client(command) => client::run(options.socket, command)
            .await
            .map_err(Error::from),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{:#}", error);
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
enum Error {
    Server(ServerError),
    Client(ClientError),
}

impl From<ServerError> for Error {
    fn from(src: ServerError) -> Self {
        Self::Server(src)
    }
}

impl From<ClientError> for Error {
    fn from(src: ClientError) -> Self {
        Self::Client(src)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Server(error) => error.fmt(f),
            Self::Client(error) => error.fmt(f),
        }
    }
}
