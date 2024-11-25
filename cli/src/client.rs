use crate::{
    format::{OptionSecondsDisplay, PeerAddrDisplay, PeerInfoDisplay, QuotaInfoDisplay},
    options::ClientCommand,
};
use futures_util::SinkExt;
use ouisync::{crypto::Password, LocalSecret, PeerAddr, PeerInfo, SetLocalSecret, ShareToken};
use ouisync_service::{
    protocol::{
        Message, MessageId, ProtocolError, QuotaInfo, RepositoryHandle, Request, Response,
        ServerPayload, UnexpectedResponse,
    },
    transport::{self, LocalClientReader, LocalClientWriter, ReadError, WriteError},
};
use std::{
    collections::BTreeMap,
    env, io,
    path::{Path, PathBuf},
    time::Duration,
};
use thiserror::Error;
use tokio::io::{stdin, stdout, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_stream::StreamExt;

pub(crate) async fn run(socket_path: PathBuf, command: ClientCommand) -> Result<(), ClientError> {
    let mut client = LocalClient::connect(&socket_path).await?;

    match command {
        ClientCommand::AddPeers { addrs } => {
            let () = client
                .invoke(Request::NetworkAddUserProvidedPeers(addrs))
                .await?;
        }
        ClientCommand::Bind { addrs } => {
            let () = client.invoke(Request::NetworkBind(addrs)).await?;
        }
        ClientCommand::Create {
            name,
            token,
            password,
            read_password,
            write_password,
        } => {
            let token = get_or_read(token, "input token").await?;
            let token = token
                .as_deref()
                .map(str::parse::<ShareToken>)
                .transpose()
                .map_err(|error| ProtocolError::new(format!("invalid share token: {error}")))?;

            let password = get_or_read(password, "input password").await?;

            let read_password = get_or_read(read_password, "input read password").await?;
            let read_secret = read_password
                .or_else(|| password.as_ref().cloned())
                .map(Password::from)
                .map(SetLocalSecret::Password);

            let write_password = get_or_read(write_password, "input write password").await?;
            let write_secret = write_password
                .or(password)
                .map(Password::from)
                .map(SetLocalSecret::Password);

            let name = name
                .or_else(|| {
                    token
                        .as_ref()
                        .map(|token| token.suggested_name().to_owned())
                })
                .ok_or_else(|| ProtocolError::new("name is missing"))?;

            let () = client
                .invoke(Request::RepositoryCreate {
                    name,
                    token,
                    read_secret,
                    write_secret,
                })
                .await?;
        }
        ClientCommand::Delete { name } => {
            let handle = client.find_repository(name).await?;
            let () = client.invoke(Request::RepositoryDelete(handle)).await?;
        }
        ClientCommand::Dht { name, enabled } => {
            let handle = client.find_repository(name).await?;

            if let Some(enabled) = enabled {
                let () = client
                    .invoke(Request::RepositorySetDhtEnabled { handle, enabled })
                    .await?;
            } else {
                let value: bool = client
                    .invoke(Request::RepositoryIsDhtEnabled(handle))
                    .await?;

                println!("{value}");
            }
        }
        ClientCommand::Expiration {
            name,
            remove,
            block,
            repository,
        } => {
            if let Some(name) = name {
                let handle = client.find_repository(name).await?;

                if remove {
                    let () = client
                        .invoke(Request::RepositorySetBlockExpiration {
                            handle,
                            value: None,
                        })
                        .await?;

                    let () = client
                        .invoke(Request::RepositorySetRepositoryExpiration {
                            handle,
                            value: None,
                        })
                        .await?;
                } else {
                    if let Some(expiration) = block {
                        let () = client
                            .invoke(Request::RepositorySetBlockExpiration {
                                handle,
                                value: Some(Duration::from_secs(expiration)),
                            })
                            .await?;
                    }

                    if let Some(expiration) = repository {
                        let () = client
                            .invoke(Request::RepositorySetRepositoryExpiration {
                                handle,
                                value: Some(Duration::from_secs(expiration)),
                            })
                            .await?;
                    }
                }

                let block: Option<Duration> = client
                    .invoke(Request::RepositoryGetBlockExpiration(handle))
                    .await?;
                let repository: Option<Duration> = client
                    .invoke(Request::RepositoryGetRepositoryExpiration(handle))
                    .await?;

                println!("block expiration:      {}", OptionSecondsDisplay(block));
                println!(
                    "repository expiration: {}",
                    OptionSecondsDisplay(repository)
                );
            } else {
                if remove {
                    let () = client
                        .invoke(Request::RepositorySetDefaultBlockExpiration { value: None })
                        .await?;

                    let () = client
                        .invoke(Request::RepositorySetDefaultRepositoryExpiration { value: None })
                        .await?;
                } else {
                    if let Some(expiration) = block {
                        let () = client
                            .invoke(Request::RepositorySetDefaultBlockExpiration {
                                value: Some(Duration::from_secs(expiration)),
                            })
                            .await?;
                    }

                    if let Some(expiration) = repository {
                        let () = client
                            .invoke(Request::RepositorySetDefaultRepositoryExpiration {
                                value: Some(Duration::from_secs(expiration)),
                            })
                            .await?;
                    }
                }

                let block = client
                    .invoke(Request::RepositoryGetDefaultBlockExpiration)
                    .await?;
                let repository = client
                    .invoke(Request::RepositoryGetDefaultRepositoryExpiration)
                    .await?;

                println!("block expiration:      {}", OptionSecondsDisplay(block));
                println!(
                    "repository expiration: {}",
                    OptionSecondsDisplay(repository)
                );
            }
        }
        ClientCommand::Export { name, output } => {
            let handle = client.find_repository(name).await?;
            let path: PathBuf = client
                .invoke(Request::RepositoryExport {
                    handle,
                    output: to_absolute(output)?,
                })
                .await?;

            println!("{}", path.display());
        }
        ClientCommand::Import {
            input,
            name,
            mode,
            force,
        } => {
            let _: RepositoryHandle = client
                .invoke(Request::RepositoryImport {
                    input: to_absolute(input)?,
                    name,
                    mode,
                    force,
                })
                .await?;
        }
        ClientCommand::ListBinds => {
            let addrs: Vec<PeerAddr> = client.invoke(Request::NetworkGetListenerAddrs).await?;

            for addr in addrs {
                println!("{}", PeerAddrDisplay(&addr));
            }
        }
        ClientCommand::ListPeers => {
            let infos: Vec<PeerInfo> = client.invoke(Request::NetworkGetPeers).await?;

            for info in infos {
                println!("{}", PeerInfoDisplay(&info));
            }
        }
        ClientCommand::ListRepositories => {
            let repos: BTreeMap<String, RepositoryHandle> =
                client.invoke(Request::RepositoryList).await?;

            for name in repos.keys() {
                println!("{name}");
            }
        }
        ClientCommand::LocalDiscovery { enabled } => {
            if let Some(enabled) = enabled {
                let () = client
                    .invoke(Request::NetworkSetLocalDiscoveryEnabled(enabled))
                    .await?;
            } else {
                let value: bool = client
                    .invoke(Request::NetworkIsLocalDiscoveryEnabled)
                    .await?;

                println!("{value}");
            }
        }
        ClientCommand::Metrics { addr } => {
            let () = client.invoke(Request::MetricsBind { addr }).await?;
        }
        ClientCommand::Mount { name } => {
            if let Some(name) = name {
                let handle = client.find_repository(name).await?;
                let path: PathBuf = client.invoke(Request::RepositoryMount(handle)).await?;

                println!("{}", path.display());
            } else {
                let handles = client.list_repositories().await?;

                for handle in handles {
                    let _: PathBuf = client.invoke(Request::RepositoryMount(handle)).await?;
                }
            }
        }
        ClientCommand::MountDir { path } => {
            if let Some(path) = path {
                let () = client.invoke(Request::RepositorySetMountDir(path)).await?;
            } else {
                let path: PathBuf = client.invoke(Request::RepositoryGetMountDir).await?;

                println!("{}", path.display());
            }
        }
        ClientCommand::Pex {
            name,
            enabled,
            send,
            recv,
        } => {
            if let Some(name) = name {
                let handle = client.find_repository(name).await?;

                if let Some(enabled) = enabled {
                    let () = client
                        .invoke(Request::RepositorySetPexEnabled { handle, enabled })
                        .await?;
                } else {
                    let value: bool = client
                        .invoke(Request::RepositoryIsPexEnabled(handle))
                        .await?;

                    println!("{value}");
                }
            } else if send.is_some() || recv.is_some() {
                if let Some(send) = send {
                    let () = client
                        .invoke(Request::NetworkSetPexSendEnabled(send))
                        .await?;
                }

                if let Some(recv) = recv {
                    let () = client
                        .invoke(Request::NetworkSetPexRecvEnabled(recv))
                        .await?;
                }
            } else {
                let send: bool = client.invoke(Request::NetworkIsPexSendEnabled).await?;
                let recv: bool = client.invoke(Request::NetworkIsPexRecvEnabled).await?;

                println!("send: {send} recv: {recv}");
            }
        }
        ClientCommand::PortForwarding { enabled } => {
            if let Some(enabled) = enabled {
                let () = client
                    .invoke(Request::NetworkSetPortForwardingEnabled(enabled))
                    .await?;
            } else {
                let value: bool = client
                    .invoke(Request::NetworkIsPortForwardingEnabled)
                    .await?;

                println!("{value}");
            }
        }
        ClientCommand::Quota {
            name,
            remove,
            value,
        } => {
            if let Some(name) = name {
                let handle = client.find_repository(name).await?;

                if remove {
                    let () = client
                        .invoke(Request::RepositorySetQuota {
                            handle,
                            quota: None,
                        })
                        .await?;
                } else if let Some(value) = value {
                    let () = client
                        .invoke(Request::RepositorySetQuota {
                            handle,
                            quota: Some(value),
                        })
                        .await?;
                } else {
                    let value: QuotaInfo =
                        client.invoke(Request::RepositoryGetQuota(handle)).await?;
                    println!("{}", QuotaInfoDisplay(&value));
                }
            } else if remove {
                let () = client
                    .invoke(Request::RepositorySetDefaultQuota { quota: None })
                    .await?;
            } else if let Some(value) = value {
                let () = client
                    .invoke(Request::RepositorySetDefaultQuota { quota: Some(value) })
                    .await?;
            } else {
                let value: QuotaInfo = client.invoke(Request::RepositoryGetDefaultQuota).await?;
                println!("{}", QuotaInfoDisplay(&value));
            }
        }
        ClientCommand::RemoteControl { addrs } => {
            let () = client.invoke(Request::RemoteControlBind { addrs }).await?;
        }
        ClientCommand::RemovePeers { addrs } => {
            let () = client
                .invoke(Request::NetworkRemoveUserProvidedPeers(addrs))
                .await?;
        }
        ClientCommand::ResetAccess { name, token } => {
            let handle = client.find_repository(name).await?;
        }
        ClientCommand::Share {
            name,
            mode,
            password,
        } => {
            let handle = client.find_repository(name).await?;
            let password = get_or_read(password, "input password").await?;
            let secret = password.map(Password::from).map(LocalSecret::Password);

            let value: String = client
                .invoke(Request::RepositoryShare {
                    handle,
                    mode,
                    secret,
                })
                .await?;

            println!("{value}");
        }
        ClientCommand::StoreDir { path } => {
            if let Some(path) = path {
                let () = client.invoke(Request::RepositorySetStoreDir(path)).await?;
            } else {
                let path: PathBuf = client.invoke(Request::RepositoryGetStoreDir).await?;
                println!("{}", path.display());
            }
        }
        ClientCommand::Unmount { name } => {
            if let Some(name) = name {
                let handle = client.find_repository(name).await?;
                let () = client.invoke(Request::RepositoryUnmount(handle)).await?;
            } else {
                let handles = client.list_repositories().await?;

                for handle in handles {
                    let () = client.invoke(Request::RepositoryUnmount(handle)).await?;
                }
            }
        }
    };

    client.close().await?;

    Ok(())
}

struct LocalClient {
    reader: LocalClientReader,
    writer: LocalClientWriter,
}

impl LocalClient {
    async fn connect(socket_path: &Path) -> Result<Self, ClientError> {
        // TODO: if the server is not running, spin it up ourselves
        match transport::connect(socket_path).await {
            Ok((reader, writer)) => Ok(Self { reader, writer }),
            Err(error) => Err(ClientError::Connect(error)),
        }
    }

    async fn invoke<T>(&mut self, request: Request) -> Result<T, ClientError>
    where
        T: TryFrom<Response, Error = UnexpectedResponse>,
    {
        self.writer
            .send(Message {
                id: MessageId::next(),
                payload: request,
            })
            .await?;

        let message = match self.reader.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => return Err(error.into()),
            None => return Err(ClientError::Disconnected),
        };

        match message.payload {
            ServerPayload::Success(response) => Ok(response.try_into()?),
            ServerPayload::Failure(error) => Err(error.into()),
            ServerPayload::Notification(_) => Err(ClientError::UnexpectedNotification),
        }
    }

    async fn find_repository(&mut self, name: String) -> Result<RepositoryHandle, ClientError> {
        self.invoke(Request::RepositoryFind(name)).await
    }

    async fn list_repositories(&mut self) -> Result<Vec<RepositoryHandle>, ClientError> {
        let repos: BTreeMap<String, RepositoryHandle> =
            self.invoke(Request::RepositoryList).await?;
        Ok(repos.into_values().collect())
    }

    async fn close(&mut self) -> Result<(), ClientError> {
        self.writer.close().await?;

        Ok(())
    }
}

/// If value is `Some("-")`, reads the value from stdin, otherwise returns it unchanged.
// TODO: support invisible input for passwords, etc.
async fn get_or_read(value: Option<String>, prompt: &str) -> Result<Option<String>, ClientError> {
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

#[derive(Error, Debug)]
pub(crate) enum ClientError {
    #[error("{0}")]
    Protocol(ProtocolError),
    #[error("failed to receive response")]
    Read(#[from] ReadError),
    #[error("failed to send request")]
    Write(#[from] WriteError),
    #[error("failed to connect to server")]
    Connect(#[source] io::Error),
    #[error("connection closed by server")]
    Disconnected,
    #[error("unexpected notification")]
    UnexpectedNotification,
    #[error("unexpected response")]
    UnexpectedResponse,
    #[error("I/O error")]
    Io(#[from] io::Error),
}

impl From<ProtocolError> for ClientError {
    fn from(src: ProtocolError) -> Self {
        Self::Protocol(src)
    }
}

impl From<UnexpectedResponse> for ClientError {
    fn from(_: UnexpectedResponse) -> Self {
        Self::UnexpectedResponse
    }
}
