mod client;
mod handler;
mod host_addr;
mod options;
mod server;
mod transport;

use self::options::{Options, Request};
use anyhow::Result;
use clap::Parser;

pub(crate) const APP_NAME: &str = "ouisync";

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::parse();

    if let Request::Serve = options.request {
        server::run(options).await
    } else {
        client::run(options).await
    }
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
