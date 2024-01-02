use crate::{
    handler::local::LocalHandler,
    options::Dirs,
    protocol::{Error, Request, Response},
    state::State,
    transport::{local::LocalClient, native::NativeClient},
};
use anyhow::Result;
use state_monitor::StateMonitor;
use std::{
    io,
    path::{Path, PathBuf},
};
use tokio::io::{stdin, stdout, AsyncBufReadExt, AsyncWriteExt, BufReader};

pub(crate) async fn run(dirs: Dirs, socket: PathBuf, request: Request) -> Result<()> {
    let client = connect(&socket, &dirs).await?;

    let request = match request {
        Request::Create {
            name,
            share_token,
            password,
            read_password,
            write_password,
        } => {
            let share_token = get_or_read(share_token, "input share token").await?;
            let password = get_or_read(password, "input password").await?;
            let read_password = get_or_read(read_password, "input read password").await?;
            let write_password = get_or_read(write_password, "input write password").await?;

            Request::Create {
                name,
                share_token,
                password,
                read_password,
                write_password,
            }
        }
        Request::Open { name, password } => {
            let password = get_or_read(password, "input password").await?;
            Request::Open { name, password }
        }
        Request::Share {
            name,
            mode,
            password,
        } => {
            let password = get_or_read(password, "input password").await?;
            Request::Share {
                name,
                mode,
                password,
            }
        }
        _ => request,
    };

    let response = client.invoke(request).await?;
    println!("{response}");

    client.close().await;

    Ok(())
}

async fn connect(path: &Path, dirs: &Dirs) -> Result<Client> {
    match LocalClient::connect(path).await {
        Ok(client) => Ok(Client::Local(client)),
        Err(error) => match error.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                let state = State::init(dirs, StateMonitor::make_root()).await?;
                let handler = LocalHandler::new(state);

                Ok(Client::Native(NativeClient::new(handler)))
            }
            _ => Err(error.into()),
        },
    }
}

enum Client {
    Local(LocalClient),
    Native(NativeClient),
}

impl Client {
    async fn invoke(&self, request: Request) -> Result<Response, Error> {
        match self {
            Self::Local(client) => client.invoke(request).await,
            Self::Native(client) => client.invoke(request).await,
        }
    }

    async fn close(&self) {
        match self {
            Self::Native(client) => client.close().await,
            Self::Local(_) => (),
        }
    }
}

/// If value is `Some("-")`, reads the value from stdin, otherwise returns it unchanged.
// TODO: support invisible input for passwords, etc.
async fn get_or_read(value: Option<String>, prompt: &str) -> Result<Option<String>> {
    if value
        .as_ref()
        .map(|value| value.trim() == "-")
        .unwrap_or(false)
    {
        let mut stdout = stdout();
        let mut stdin = BufReader::new(stdin());

        // Read from stdin
        stdout.write_all(prompt.as_bytes()).await?;
        stdout.write_all(b": ").await?;
        stdout.flush().await?;

        let mut value = String::new();
        stdin.read_line(&mut value).await?;

        Ok(Some(value).filter(|s| !s.is_empty()))
    } else {
        Ok(value)
    }
}
