use crate::{
    format::{OptionSecondsDisplay, PeerAddrDisplay, PeerInfoDisplay, QuotaInfoDisplay},
    options::{ClientCommand, MirrorCommand, StoreDirsCommand},
};
use futures_util::SinkExt;
use ouisync::{crypto::Password, LocalSecret, PeerAddr, PeerInfo, SetLocalSecret, ShareToken};
use ouisync_service::{
    protocol::{
        ErrorCode, Message, MessageId, ProtocolError, QuotaInfo, RepositoryHandle, Request,
        Response, ResponseResult, UnexpectedResponse,
    },
    transport::{
        local::{self, LocalClientReader, LocalClientWriter, LocalEndpoint},
        ClientError,
    },
};
use std::{collections::BTreeMap, env, io, net::SocketAddr, path::PathBuf, time::Duration};
use tokio::io::{stdin, stdout, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_stream::StreamExt;

pub(crate) async fn run(config_path: PathBuf, command: ClientCommand) -> Result<(), ClientError> {
    let endpoint = ouisync_service::local_endpoint(&config_path).await?;
    let mut client = LocalClient::connect(endpoint).await?;

    match command {
        ClientCommand::AddPeers { addrs } => {
            let () = client
                .invoke(Request::SessionAddUserProvidedPeers { addrs })
                .await?;
        }
        ClientCommand::Bind { addrs, disable } => {
            if disable {
                let () = client
                    .invoke(Request::SessionBindNetwork { addrs: vec![] })
                    .await?;
            } else if !addrs.is_empty() {
                let () = client.invoke(Request::SessionBindNetwork { addrs }).await?;
            }

            let addrs: Vec<PeerAddr> = client.invoke(Request::SessionGetLocalListenerAddrs).await?;

            for addr in addrs {
                println!("{}", PeerAddrDisplay(&addr));
            }
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
                .map_err(|error| {
                    ProtocolError::new(
                        ErrorCode::InvalidInput,
                        format!("invalid share token: {error}"),
                    )
                })?;

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
                .ok_or_else(|| ProtocolError::new(ErrorCode::InvalidInput, "name is missing"))?;

            let _: RepositoryHandle = client
                .invoke(Request::SessionCreateRepository {
                    path: name.into(),
                    token,
                    read_secret,
                    write_secret,
                    sync_enabled: true,
                    dht_enabled: false,
                    pex_enabled: false,
                })
                .await?;
        }
        ClientCommand::Delete { name } => {
            let repo = client.find_repository(name).await?;
            let () = client.invoke(Request::RepositoryDelete { repo }).await?;
        }
        ClientCommand::Dht { name, enabled } => {
            let repo = client.find_repository(name).await?;

            if let Some(enabled) = enabled {
                let () = client
                    .invoke(Request::RepositorySetDhtEnabled { repo, enabled })
                    .await?;
            } else {
                let value: bool = client
                    .invoke(Request::RepositoryIsDhtEnabled { repo })
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
                let repo = client.find_repository(name).await?;

                if remove {
                    let () = client
                        .invoke(Request::RepositorySetBlockExpiration { repo, value: None })
                        .await?;

                    let () = client
                        .invoke(Request::RepositorySetExpiration { repo, value: None })
                        .await?;
                } else {
                    if let Some(expiration) = block {
                        let () = client
                            .invoke(Request::RepositorySetBlockExpiration {
                                repo,
                                value: Some(Duration::from_secs(expiration)),
                            })
                            .await?;
                    }

                    if let Some(expiration) = repository {
                        let () = client
                            .invoke(Request::RepositorySetExpiration {
                                repo,
                                value: Some(Duration::from_secs(expiration)),
                            })
                            .await?;
                    }
                }

                let block: Option<Duration> = client
                    .invoke(Request::RepositoryGetBlockExpiration { repo })
                    .await?;
                let repository: Option<Duration> = client
                    .invoke(Request::RepositoryGetExpiration { repo })
                    .await?;

                println!("block expiration:      {}", OptionSecondsDisplay(block));
                println!(
                    "repository expiration: {}",
                    OptionSecondsDisplay(repository)
                );
            } else {
                if remove {
                    let () = client
                        .invoke(Request::SessionSetDefaultBlockExpiration { value: None })
                        .await?;

                    let () = client
                        .invoke(Request::SessionSetDefaultRepositoryExpiration { value: None })
                        .await?;
                } else {
                    if let Some(expiration) = block {
                        let () = client
                            .invoke(Request::SessionSetDefaultBlockExpiration {
                                value: Some(Duration::from_secs(expiration)),
                            })
                            .await?;
                    }

                    if let Some(expiration) = repository {
                        let () = client
                            .invoke(Request::SessionSetDefaultRepositoryExpiration {
                                value: Some(Duration::from_secs(expiration)),
                            })
                            .await?;
                    }
                }

                let block = client
                    .invoke(Request::SessionGetDefaultBlockExpiration)
                    .await?;
                let repository = client
                    .invoke(Request::SessionGetDefaultRepositoryExpiration)
                    .await?;

                println!("block expiration:      {}", OptionSecondsDisplay(block));
                println!(
                    "repository expiration: {}",
                    OptionSecondsDisplay(repository)
                );
            }
        }
        ClientCommand::Export { name, output } => {
            let repo = client.find_repository(name).await?;
            let path: PathBuf = client
                .invoke(Request::RepositoryExport {
                    repo,
                    output_path: to_absolute(output)?,
                })
                .await?;

            println!("{}", path.display());
        }
        ClientCommand::ListPeers => {
            let infos: Vec<PeerInfo> = client.invoke(Request::SessionGetPeers).await?;

            for info in infos {
                println!("{}", PeerInfoDisplay(&info));
            }
        }
        ClientCommand::ListRepositories => {
            let repos: BTreeMap<PathBuf, RepositoryHandle> =
                client.invoke(Request::SessionListRepositories).await?;

            for path in repos.keys() {
                println!("{}", path.display());
            }
        }
        ClientCommand::LocalDiscovery { enabled } => {
            if let Some(enabled) = enabled {
                let () = client
                    .invoke(Request::SessionSetLocalDiscoveryEnabled { enabled })
                    .await?;
            } else {
                let value: bool = client
                    .invoke(Request::SessionIsLocalDiscoveryEnabled)
                    .await?;

                println!("{value}");
            }
        }
        ClientCommand::Metrics { addr, disable } => {
            if disable {
                let () = client
                    .invoke(Request::SessionBindMetrics { addr: None })
                    .await?;
            } else if let Some(addr) = addr {
                let () = client
                    .invoke(Request::SessionBindMetrics { addr: Some(addr) })
                    .await?;
            }

            let addr: Option<SocketAddr> = client
                .invoke(Request::SessionGetMetricsListenerAddr)
                .await?;

            if let Some(addr) = addr {
                println!("{addr}");
            }
        }
        ClientCommand::Mirror {
            command,
            name,
            host,
        } => {
            let repo = client.find_repository(name).await?;

            match command {
                MirrorCommand::Create => {
                    let () = client
                        .invoke(Request::RepositoryCreateMirror { repo, host })
                        .await?;
                }
                MirrorCommand::Delete => {
                    let () = client
                        .invoke(Request::RepositoryDeleteMirror { repo, host })
                        .await?;
                }
                MirrorCommand::Exists => {
                    let value: bool = client
                        .invoke(Request::RepositoryMirrorExists { repo, host })
                        .await?;

                    println!("{value}");
                }
            }
        }
        ClientCommand::Mount { name } => {
            if let Some(name) = name {
                let repo = client.find_repository(name).await?;
                let path: PathBuf = client.invoke(Request::RepositoryMount { repo }).await?;

                println!("{}", path.display());
            } else {
                let repos = client.list_repositories().await?;
                for repo in repos {
                    let _: PathBuf = client.invoke(Request::RepositoryMount { repo }).await?;
                }
            }
        }
        ClientCommand::MountDir { path } => {
            if let Some(path) = path {
                let () = client
                    .invoke(Request::SessionSetMountRoot { path: Some(path) })
                    .await?;
            } else {
                let path: Option<PathBuf> = client.invoke(Request::SessionGetMountRoot).await?;

                if let Some(path) = path {
                    println!("{}", path.display());
                }
            }
        }
        ClientCommand::Open { path, password } => {
            let password = get_or_read(password, "input password").await?;
            let local_secret = password.map(Password::from).map(LocalSecret::Password);

            let _: RepositoryHandle = client
                .invoke(Request::SessionOpenRepository { path, local_secret })
                .await?;
        }
        ClientCommand::Pex {
            name,
            enabled,
            send,
            recv,
        } => {
            if let Some(name) = name {
                let repo = client.find_repository(name).await?;

                if let Some(enabled) = enabled {
                    let () = client
                        .invoke(Request::RepositorySetPexEnabled { repo, enabled })
                        .await?;
                } else {
                    let value: bool = client
                        .invoke(Request::RepositoryIsPexEnabled { repo })
                        .await?;

                    println!("{value}");
                }
            } else if send.is_some() || recv.is_some() {
                if let Some(send) = send {
                    let () = client
                        .invoke(Request::SessionSetPexSendEnabled { enabled: send })
                        .await?;
                }

                if let Some(recv) = recv {
                    let () = client
                        .invoke(Request::SessionSetPexRecvEnabled { enabled: recv })
                        .await?;
                }
            } else {
                let send: bool = client.invoke(Request::SessionIsPexSendEnabled).await?;
                let recv: bool = client.invoke(Request::SessionIsPexRecvEnabled).await?;

                println!("send: {send} recv: {recv}");
            }
        }
        ClientCommand::PortForwarding { enabled } => {
            if let Some(enabled) = enabled {
                let () = client
                    .invoke(Request::SessionSetPortForwardingEnabled { enabled })
                    .await?;
            } else {
                let value: bool = client
                    .invoke(Request::SessionIsPortForwardingEnabled)
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
                let repo = client.find_repository(name).await?;

                if remove {
                    let () = client
                        .invoke(Request::RepositorySetQuota { repo, value: None })
                        .await?;
                } else if let Some(value) = value {
                    let () = client
                        .invoke(Request::RepositorySetQuota {
                            repo,
                            value: Some(value),
                        })
                        .await?;
                } else {
                    let value: QuotaInfo =
                        client.invoke(Request::RepositoryGetQuota { repo }).await?;
                    println!("{}", QuotaInfoDisplay(&value));
                }
            } else if remove {
                let () = client
                    .invoke(Request::SessionSetDefaultQuota { value: None })
                    .await?;
            } else if let Some(value) = value {
                let () = client
                    .invoke(Request::SessionSetDefaultQuota { value: Some(value) })
                    .await?;
            } else {
                let value: QuotaInfo = client.invoke(Request::SessionGetDefaultQuota).await?;
                println!("{}", QuotaInfoDisplay(&value));
            }
        }
        ClientCommand::RemoteControl { addr, disable } => {
            if disable {
                let _: u16 = client
                    .invoke(Request::SessionBindRemoteControl { addr: None })
                    .await?;
            } else if let Some(addr) = addr {
                let _: u16 = client
                    .invoke(Request::SessionBindRemoteControl { addr: Some(addr) })
                    .await?;
            }

            let addr: Option<SocketAddr> = client
                .invoke(Request::SessionGetRemoteControlListenerAddr)
                .await?;

            if let Some(addr) = addr {
                println!("{addr}");
            }
        }
        ClientCommand::RemovePeers { addrs } => {
            let () = client
                .invoke(Request::SessionRemoveUserProvidedPeers { addrs })
                .await?;
        }
        ClientCommand::ResetAccess { name, token } => {
            let repo = client.find_repository(name).await?;
            let token = token.parse().map_err(|_| ClientError::InvalidArgument)?;

            let () = client
                .invoke(Request::RepositoryResetAccess { repo, token })
                .await?;
        }
        ClientCommand::Share {
            name,
            mode,
            password,
        } => {
            let repo = client.find_repository(name).await?;
            let password = get_or_read(password, "input password").await?;
            let local_secret = password.map(Password::from).map(LocalSecret::Password);

            let value: ShareToken = client
                .invoke(Request::RepositoryShare {
                    repo,
                    access_mode: mode,
                    local_secret,
                })
                .await?;

            println!("{value}");
        }
        ClientCommand::StoreDirs { command } => match command {
            StoreDirsCommand::Insert { paths } => {
                let () = client
                    .invoke(Request::SessionInsertStoreDirs { paths })
                    .await?;
            }
            StoreDirsCommand::Remove { paths } => {
                let () = client
                    .invoke(Request::SessionRemoveStoreDirs { paths })
                    .await?;
            }
            StoreDirsCommand::Set { paths } => {
                let () = client
                    .invoke(Request::SessionSetStoreDirs { paths })
                    .await?;
            }
            StoreDirsCommand::List => {
                let paths: Vec<PathBuf> = client.invoke(Request::SessionGetStoreDirs).await?;

                for path in paths {
                    println!("{}", path.display());
                }
            }
        },
        ClientCommand::Unmount { name } => {
            if let Some(name) = name {
                let repo = client.find_repository(name).await?;
                let () = client.invoke(Request::RepositoryUnmount { repo }).await?;
            } else {
                let repos = client.list_repositories().await?;
                for repo in repos {
                    let () = client.invoke(Request::RepositoryUnmount { repo }).await?;
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
    async fn connect(endpoint: LocalEndpoint) -> Result<Self, ClientError> {
        let (reader, writer) = local::connect(endpoint).await?;
        Ok(Self { reader, writer })
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
            ResponseResult::Success(response) => Ok(response.try_into()?),
            ResponseResult::Failure(error) => Err(error.into()),
        }
    }

    async fn find_repository(&mut self, name: String) -> Result<RepositoryHandle, ClientError> {
        self.invoke(Request::SessionFindRepository { name }).await
    }

    async fn list_repositories(&mut self) -> Result<Vec<RepositoryHandle>, ClientError> {
        let repos: BTreeMap<PathBuf, RepositoryHandle> =
            self.invoke(Request::SessionListRepositories).await?;
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
