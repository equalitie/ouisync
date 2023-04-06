use crate::{
    handler::{Handler, State},
    host_addr::HostAddr,
    options::{Dirs, Options, Request, Response},
    transport::{local::LocalClient, native::NativeClient, remote::RemoteClient},
};
use anyhow::Result;
use ouisync_bridge::transport::Client;
use ouisync_lib::StateMonitor;
use std::{io, sync::Arc};
use tokio::io::{stdin, stdout, AsyncBufReadExt, AsyncWriteExt, BufReader};

pub(crate) async fn run(options: Options) -> Result<()> {
    let client = connect(options.host, &options.dirs).await?;

    let request = options.request;
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

async fn connect(
    addr: HostAddr,
    dirs: &Dirs,
) -> io::Result<Box<dyn Client<Request = Request, Response = Response>>> {
    match addr {
        HostAddr::Local(addr) => match LocalClient::connect(addr.as_str()).await {
            Ok(client) => Ok(Box::new(client)),
            Err(error) => match error.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                    let state = State::new(dirs, StateMonitor::make_root()).await;
                    let state = Arc::new(state);
                    let handler = Handler::new(state);

                    Ok(Box::new(NativeClient::new(handler)))
                }
                _ => Err(error),
            },
        },
        HostAddr::Remote(addr) => Ok(Box::new(RemoteClient::connect(addr).await?)),
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
