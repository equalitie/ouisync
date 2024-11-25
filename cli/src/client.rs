use crate::{format::print_response, options::ClientCommand};
use futures_util::SinkExt;
use ouisync::{crypto::Password, LocalSecret, SetLocalSecret, ShareToken};
use ouisync_service::{
    protocol::{
        Message, MessageId, ProtocolError, RepositoryHandle, Request, Response, ServerPayload,
        UnexpectedResponse,
    },
    transport::{self, LocalClientReader, LocalClientWriter, ReadError, WriteError},
};
use std::{
    env, io,
    path::{Path, PathBuf},
};
use thiserror::Error;
use tokio::io::{stdin, stdout, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_stream::StreamExt;

pub(crate) async fn run(socket_path: PathBuf, command: ClientCommand) -> Result<(), ClientError> {
    let mut client = LocalClient::connect(&socket_path).await?;

    let response = match command {
        ClientCommand::AddPeers { addrs } => {
            client
                .invoke(Request::NetworkAddUserProvidedPeers(addrs))
                .await?
        }
        ClientCommand::Bind { addrs } => client.invoke(Request::NetworkBind(addrs)).await?,
        ClientCommand::Create {
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
                    share_token
                        .as_ref()
                        .map(|token| token.suggested_name().to_owned())
                })
                .ok_or_else(|| ProtocolError::new("name is missing"))?;

            client
                .invoke(Request::RepositoryCreate {
                    name,
                    share_token,
                    read_secret,
                    write_secret,
                })
                .await?
        }
        ClientCommand::Delete { name } => {
            let handle = client.find_repository(name).await?;
            client.invoke(Request::RepositoryDelete(handle)).await?
        }
        ClientCommand::Dht { name, enabled } => {
            let handle = client.find_repository(name).await?;

            if let Some(enabled) = enabled {
                client
                    .invoke(Request::RepositorySetDhtEnabled { handle, enabled })
                    .await?
            } else {
                client
                    .invoke(Request::RepositoryIsDhtEnabled(handle))
                    .await?
            }
        }
        ClientCommand::Export { name, output } => {
            let handle = client.find_repository(name).await?;
            client
                .invoke(Request::RepositoryExport {
                    handle,
                    output: to_absolute(output)?,
                })
                .await?
        }
        ClientCommand::Import {
            input,
            name,
            mode,
            force,
        } => {
            client
                .invoke(Request::RepositoryImport {
                    input: to_absolute(input)?,
                    name,
                    mode,
                    force,
                })
                .await?
        }
        ClientCommand::ListBinds => client.invoke(Request::NetworkGetListenerAddrs).await?,
        ClientCommand::ListPeers => client.invoke(Request::NetworkGetPeers).await?,
        ClientCommand::ListRepositories => client.invoke(Request::RepositoryList).await?,
        ClientCommand::LocalDiscovery { enabled } => {
            if let Some(enabled) = enabled {
                client
                    .invoke(Request::NetworkSetLocalDiscoveryEnabled(enabled))
                    .await?
            } else {
                client
                    .invoke(Request::NetworkIsLocalDiscoveryEnabled)
                    .await?
            }
        }
        ClientCommand::Metrics { addr } => client.invoke(Request::MetricsBind { addr }).await?,
        ClientCommand::Mount { name } => {
            if let Some(name) = name {
                let handle = client.find_repository(name).await?;
                client.invoke(Request::RepositoryMount(handle)).await?
            } else {
                let handles = client.list_repositories().await?;

                for handle in handles {
                    client.invoke(Request::RepositoryMount(handle)).await?;
                }

                Response::None
            }
        }
        ClientCommand::MountDir { path } => {
            let request = if let Some(path) = path {
                Request::RepositorySetMountDir(path)
            } else {
                Request::RepositoryGetMountDir
            };

            client.invoke(request).await?
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
                    client
                        .invoke(Request::RepositorySetPexEnabled { handle, enabled })
                        .await?
                } else {
                    client
                        .invoke(Request::RepositoryIsPexEnabled(handle))
                        .await?
                }
            } else if send.is_some() || recv.is_some() {
                if let Some(send) = send {
                    let () = client
                        .invoke(Request::NetworkSetPexSendEnabled(send))
                        .await?
                        .try_into()?;
                }

                if let Some(recv) = recv {
                    let () = client
                        .invoke(Request::NetworkSetPexRecvEnabled(recv))
                        .await?
                        .try_into()?;
                }

                Response::None
            } else {
                let send: bool = client
                    .invoke(Request::NetworkIsPexSendEnabled)
                    .await?
                    .try_into()?;
                let recv: bool = client
                    .invoke(Request::NetworkIsPexRecvEnabled)
                    .await?
                    .try_into()?;

                Response::String(format!("send: {send} recv: {recv}",))
            }
        }
        ClientCommand::PortForwarding { enabled } => {
            if let Some(enabled) = enabled {
                client
                    .invoke(Request::NetworkSetPortForwardingEnabled(enabled))
                    .await?
            } else {
                client
                    .invoke(Request::NetworkIsPortForwardingEnabled)
                    .await?
            }
        }
        ClientCommand::Quota {
            name,
            remove,
            value,
        } => {
            let request = if let Some(name) = name {
                let handle = client.find_repository(name).await?;

                if remove {
                    Request::RepositorySetQuota {
                        handle,
                        quota: None,
                    }
                } else if let Some(value) = value {
                    Request::RepositorySetQuota {
                        handle,
                        quota: Some(value),
                    }
                } else {
                    Request::RepositoryGetQuota(handle)
                }
            } else if remove {
                Request::RepositorySetDefaultQuota { quota: None }
            } else if let Some(value) = value {
                Request::RepositorySetDefaultQuota { quota: Some(value) }
            } else {
                Request::RepositoryGetDefaultQuota
            };

            client.invoke(request).await?
        }
        ClientCommand::RemoteControl { addrs } => {
            client.invoke(Request::RemoteControlBind { addrs }).await?
        }
        ClientCommand::RemovePeers { addrs } => {
            client
                .invoke(Request::NetworkRemoveUserProvidedPeers(addrs))
                .await?
        }
        ClientCommand::Share {
            name,
            mode,
            password,
        } => {
            let handle = client.find_repository(name).await?;
            let password = get_or_read(password, "input password").await?;
            let secret = password.map(Password::from).map(LocalSecret::Password);

            client
                .invoke(Request::RepositoryShare {
                    handle,
                    mode,
                    secret,
                })
                .await?
        }
        ClientCommand::StoreDir { path } => {
            let request = if let Some(path) = path {
                Request::RepositorySetStoreDir(path)
            } else {
                Request::RepositoryGetStoreDir
            };

            client.invoke(request).await?
        }
        ClientCommand::Unmount { name } => {
            if let Some(name) = name {
                let handle = client.find_repository(name).await?;
                client.invoke(Request::RepositoryUnmount(handle)).await?
            } else {
                let handles = client.list_repositories().await?;

                for handle in handles {
                    client.invoke(Request::RepositoryUnmount(handle)).await?;
                }

                Response::None
            }
        }
    };

    print_response(&response);

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

    async fn invoke(&mut self, request: Request) -> Result<Response, ClientError> {
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
            ServerPayload::Success(response) => Ok(response),
            ServerPayload::Failure(error) => Err(error.into()),
            ServerPayload::Notification(_) => Err(ClientError::UnexpectedNotification),
        }
    }

    async fn find_repository(&mut self, name: String) -> Result<RepositoryHandle, ClientError> {
        Ok(self
            .invoke(Request::RepositoryFind(name))
            .await?
            .try_into()?)
    }

    async fn list_repositories(&mut self) -> Result<Vec<RepositoryHandle>, ClientError> {
        match self.invoke(Request::RepositoryList).await? {
            Response::Repositories(map) => Ok(map.into_values().collect()),
            _ => Err(ClientError::UnexpectedResponse),
        }
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
