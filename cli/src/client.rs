use crate::{
    host_addr::HostAddr,
    options::{Command, Options},
};
use anyhow::Result;
use ouisync_bridge::{
    protocol::Request,
    transport::{local::LocalClient, native::NativeClient, remote::RemoteClient, Client},
    ServerState,
};
use ouisync_lib::{PeerAddr, ShareToken, StateMonitor};
use std::{io, net::SocketAddr, path::Path, path::PathBuf, sync::Arc};
use tokio::io::{stdin, stdout, AsyncBufReadExt, AsyncWriteExt, BufReader};

pub(crate) async fn run(options: Options) -> Result<()> {
    let client = connect(options.host, &options.config_dir).await?;

    match options.command {
        Command::Serve => unreachable!(), // handled already in `main`
        Command::Create {
            name,
            share_token,
            password,
            read_password,
            write_password,
        } => {
            let share_token = get_or_read(share_token, "input share token").await?;
            let share_token = share_token
                .as_deref()
                .map(str::parse::<ShareToken>)
                .transpose()?;

            let password = get_or_read(password, "input password").await?;

            let read_password = get_or_read(read_password, "input read password").await?;
            let read_password = read_password.or_else(|| password.as_ref().cloned());

            let write_password = get_or_read(write_password, "input write password").await?;
            let write_password = write_password.or(password);

            let name = match (name, &share_token) {
                (Some(name), _) => name,
                (None, Some(token)) => token.suggested_name().into_owned(),
                (None, None) => unreachable!(),
            };

            let path = repository_path(&options.data_dir, &name).try_into()?;

            client
                .invoke(Request::RepositoryCreate {
                    path,
                    read_password,
                    write_password,
                    share_token,
                })
                .await?;

            println!("repository created");
        }
        Command::Delete { .. } => todo!(),
        Command::Share {
            name,
            mode,
            password,
        } => {
            let password = get_or_read(password, "input password").await?;
            let repository = client
                .invoke(Request::RepositoryOpen {
                    path: repository_path(&options.data_dir, &name).try_into()?,
                    password: None,
                    // TODO: scope: Scope::Client,
                })
                .await?
                .try_into()
                .unwrap();
            let token: String = client
                .invoke(Request::RepositoryCreateShareToken {
                    repository,
                    password,
                    access_mode: mode,
                    name: Some(name),
                })
                .await?
                .try_into()
                .unwrap();

            println!("{token}");
        }
        Command::Bind { addrs } => {
            let mut quic_v4 = None;
            let mut quic_v6 = None;
            let mut tcp_v4 = None;
            let mut tcp_v6 = None;

            for addr in addrs {
                match addr {
                    PeerAddr::Quic(SocketAddr::V4(addr)) => quic_v4 = Some(addr),
                    PeerAddr::Quic(SocketAddr::V6(addr)) => quic_v6 = Some(addr),
                    PeerAddr::Tcp(SocketAddr::V4(addr)) => tcp_v4 = Some(addr),
                    PeerAddr::Tcp(SocketAddr::V6(addr)) => tcp_v6 = Some(addr),
                }
            }

            client
                .invoke(Request::NetworkBind {
                    quic_v4,
                    quic_v6,
                    tcp_v4,
                    tcp_v6,
                })
                .await?;
        }
    }

    client.close().await;

    Ok(())
}

async fn connect(addr: HostAddr, config_dir: &Path) -> io::Result<Box<dyn Client>> {
    match addr {
        HostAddr::Local(addr) => match LocalClient::connect(addr).await {
            Ok(client) => Ok(Box::new(client)),
            Err(error) => match error.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                    let root_monitor = StateMonitor::make_root();
                    let state = ServerState::new(config_dir.into(), root_monitor);
                    let state = Arc::new(state);

                    Ok(Box::new(NativeClient::new(state)))
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

fn repository_path(data_dir: &Path, name: &str) -> PathBuf {
    data_dir
        .join("repositories")
        .join(name)
        .with_extension("db")
}
