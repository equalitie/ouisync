mod host_addr;
mod options;
mod path;

use self::options::{Command, Options};
use anyhow::Result;
use clap::Parser;
use ouisync_bridge::{
    logger,
    transport::{local::LocalServer, Server},
    ServerState,
};
use ouisync_lib::StateMonitor;
use std::{io, sync::Arc};
use tokio::select;

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

    select! {
        _ = server.run(state) => (),
        result = terminated() => result?,
    }

    Ok(())
}

async fn client(_options: Options) -> Result<()> {
    todo!()
}

// Wait until the program is terminated.
#[cfg(unix)]
async fn terminated() -> io::Result<()> {
    use tokio::signal::unix::{signal, SignalKind};

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

async fn create_repository(
    name: &str,
    device_id: DeviceId,
    secrets: AccessSecrets,
    options: &Options,
) -> Result<Repository> {
    let db = RepositoryDb::create(options.repository_path(name.as_ref())?).await?;
    let local_key = secret_to_key(&db, options.secret_for_repo(name)?).await?;
    // TODO: In CLI we currently support only a single (optional) user secret for writing and
    // reading, but the code allows for having them distinct.
    let access = Access::new(local_key.clone(), local_key, secrets);
    let repo = Repository::create(db, device_id, access).await?;
    Ok(repo)
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

    for name in &options.create {
        let repo =
            create_repository(name, device_id, AccessSecrets::random_write(), &options).await?;

        repos.insert(name.clone(), repo);
    }

    // Print share tokens
    let mut share_file = if let Some(path) = &options.share_file {
        Some(File::create(path).await?)
    } else {
        None
    };

    for Named { name, value } in &options.share {
        let secrets = if let Some(repo) = repos.get(name) {
            repo.secrets().with_mode(*value)
        } else {
            Repository::open(
                options.repository_path(name)?,
                device_id,
                options.secret_for_repo(name)?,
            )
            .await?
            .secrets()
            .with_mode(*value)
        };

        let token = ShareToken::from(secrets).with_name(name);

        if let Some(file) = &mut share_file {
            file.write_all(token.to_string().as_bytes()).await?;
            file.write_all(b"\n").await?;
        } else {
            println!("{}", token);
        }
    }

    if let Some(mut file) = share_file {
        file.flush().await?;
    }

    // Accept share tokens
    let accept_file_tokens = if let Some(path) = &options.accept_file {
        options::read_share_tokens_from_file(path).await?
    } else {
        vec![]
    };

    for token in options.accept.iter().chain(&accept_file_tokens) {
        let name = token.suggested_name();

        if let Entry::Vacant(entry) = repos.entry(name.as_ref().to_owned()) {
            let repo =
                create_repository(name.as_ref(), device_id, token.secrets().clone(), &options)
                    .await?;

            entry.insert(repo);

            tracing::info!("share token accepted: {}", token);
        } else {
            return Err(format_err!(
                "can't accept share token for repository {:?} - already exists",
                name
            ));
        }
    }

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

    terminated().await?;

    time::timeout(Duration::from_secs(1), network.handle().shutdown())
        .await
        .unwrap_or(());

    Ok(())
}


*/
