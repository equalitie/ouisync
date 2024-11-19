use futures_util::SinkExt;
use ouisync_service::{
    protocol::{Message, MessageId, Request, ServerError, ServerPayload},
    transport::{self, LocalClientReader, LocalClientWriter},
};
use std::{
    env, io,
    path::{Path, PathBuf},
};
use tokio::io::{stdin, stdout, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_stream::StreamExt;

pub(crate) async fn run(socket_path: PathBuf, request: Request) -> Result<(), ServerError> {
    let (mut reader, mut writer) = connect(&socket_path).await?;

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
        Request::Export { name, output } => Request::Export {
            name,
            output: to_absolute(output)?,
        },
        Request::Import {
            name,
            mode,
            force,
            input,
        } => Request::Import {
            name,
            mode,
            force,
            input: to_absolute(input)?,
        },

        _ => request,
    };

    writer
        .send(Message {
            id: MessageId::next(),
            payload: request,
        })
        .await?;

    let message = match reader.next().await {
        Some(Ok(message)) => message,
        Some(Err(error)) => return Err(error.into()),
        None => return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into()),
    };

    writer.close().await?;

    match message.payload {
        ServerPayload::Success(response) => {
            println!("{}", response);
            Ok(())
        }
        ServerPayload::Failure(error) => Err(error),
        ServerPayload::Notification(notification) => Err(ServerError::new(format!(
            "unexpected notification: {:?}",
            notification
        ))),
    }
}

async fn connect(socket_path: &Path) -> io::Result<(LocalClientReader, LocalClientWriter)> {
    // TODO: if the server is not running, spin it up ourselves
    transport::connect(socket_path).await
}

/// If value is `Some("-")`, reads the value from stdin, otherwise returns it unchanged.
// TODO: support invisible input for passwords, etc.
async fn get_or_read(value: Option<String>, prompt: &str) -> io::Result<Option<String>> {
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

fn to_absolute(path: PathBuf) -> Result<PathBuf, io::Error> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}
