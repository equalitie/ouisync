mod host_addr;
mod options;
mod path;

use self::{
    host_addr::HostAddr,
    options::{Command, Options},
};
use anyhow::Result;
use clap::Parser;
use ouisync_bridge::{
    logger,
    protocol::Request,
    transport::{
        local::{LocalClient, LocalServer},
        native::NativeClient,
        remote::RemoteClient,
        Client, DefaultHandler,
    },
    ServerState,
};
use ouisync_lib::{ShareToken, StateMonitor};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    io::{stdin, stdout, AsyncBufReadExt, AsyncWriteExt, BufReader},
    task,
};

pub(crate) const APP_NAME: &str = "ouisync";

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::parse();

    if let Command::Serve = options.command {
        server(options).await
    } else {
        client(options).await
    }
}

async fn server(options: Options) -> Result<()> {
    let root_monitor = StateMonitor::make_root();
    let _logger = logger::new(root_monitor.clone());

    let state = ServerState::new(options.config_dir.into(), root_monitor);
    let state = Arc::new(state);

    let server = LocalServer::bind(host_addr::default_local())?;
    task::spawn(server.run(DefaultHandler::new(state.clone())));

    terminated().await?;
    state.close().await;

    Ok(())
}

async fn client(options: Options) -> Result<()> {
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

// Wait until the program is terminated.
#[cfg(unix)]
async fn terminated() -> io::Result<()> {
    use tokio::{
        select,
        signal::unix::{signal, SignalKind},
    };

    // Wait for SIGINT or SIGTERM
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;

    select! {
        _ = interrupt.recv() => (),
        _ = terminate.recv() => (),
    }

    Ok(())
}

#[cfg(not(unix))]
async fn terminated() -> io::Result<()> {
    tokio::signal::ctrl_c().await
}

/*
use self::options::{Named, Options};
use ouisync_lib::{
    crypto::cipher::SecretKey,
    device_id::{self, DeviceId},
    network::Network,
    Access, AccessSecrets, ConfigStore, LocalSecret, Repository, RepositoryDb, ShareToken,
};
use std::{
    collections::{hash_map::Entry, HashMap},
    io,
    time::Duration,
};
use tokio::{fs::File, io::AsyncWriteExt, time};


async fn secret_to_key(
    db: &RepositoryDb,
    secret: Option<LocalSecret>,
) -> Result<Option<SecretKey>> {
    let secret = if let Some(secret) = secret {
        secret
    } else {
        return Ok(None);
    };

    let key = match secret {
        LocalSecret::Password(pwd) => db.password_to_key(pwd).await?,
        LocalSecret::SecretKey(key) => key,
    };
    Ok(Some(key))
}

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::parse();

    if options.print_dirs {
        println!("data:   {}", options.data_dir()?.display());
        println!("config: {}", options.config_dir()?.display());
        return Ok(());
    }

    init_log();

    let config = ConfigStore::new(options.config_dir()?);
    let device_id = device_id::get_or_create(&config).await?;

    // Create repositories
    let mut repos = HashMap::new();

    // Start the network
    let network = Network::new(config);
    let network_handle = network.handle();

    network_handle.bind(&options.bind).await;

    if !options.disable_upnp {
        network.enable_port_forwarding();
    }

    if !options.disable_local_discovery {
        network.enable_local_discovery();
    }

    for peer in &options.peers {
        network.add_user_provided_peer(peer);
    }

    // Mount repositories
    let mut repo_guards = Vec::new();
    for Named { name, value } in &options.mount {
        let repo = if let Some(repo) = repos.remove(name) {
            repo
        } else {
            Repository::open(
                options.repository_path(name)?,
                device_id,
                options.secret_for_repo(name)?,
            )
            .await?
        };

        let registration = network_handle.register(repo.store().clone());

        if !options.disable_dht {
            registration.enable_dht();
        }

        if !options.disable_pex {
            registration.enable_pex();
        }

        let mount_guard =
            ouisync_vfs::mount(tokio::runtime::Handle::current(), repo, value.clone())?;

        repo_guards.push((mount_guard, registration));
    }

    if options.print_port {
        if let Some(addr) = network.tcp_listener_local_addr_v4() {
            // Be carefull when changing this output as it is used by tests.
            println!("Listening on TCP IPv4 port {}", addr.port());
        }
        if let Some(addr) = network.tcp_listener_local_addr_v6() {
            println!("Listening on TCP IPv6 port {}", addr.port());
        }
        if let Some(addr) = network.quic_listener_local_addr_v4() {
            println!("Listening on QUIC/UDP IPv4 port {}", addr.port());
        }
        if let Some(addr) = network.quic_listener_local_addr_v6() {
            println!("Listening on QUIC/UDP IPv6 port {}", addr.port());
        }
    }

    if options.print_device_id {
        println!("Device ID is {}", device_id);
    }

    Ok(())
}


*/
