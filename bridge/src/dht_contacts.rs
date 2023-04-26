use async_trait::async_trait;
use ouisync_lib::network::dht_discovery::DhtContactsStoreTrait;
use std::{
    collections::HashSet,
    io,
    net::{SocketAddrV4, SocketAddrV6},
    path::{Path, PathBuf},
};
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
};

pub struct Store {
    path_v4: PathBuf,
    path_v6: PathBuf,
}

impl Store {
    pub fn new<P: AsRef<Path>>(store_dir: P) -> Self {
        Self {
            path_v4: store_dir.as_ref().join("dht_contacts_v4.txt"),
            path_v6: store_dir.as_ref().join("dht_contacts_v6.txt"),
        }
    }
}

#[async_trait]
impl DhtContactsStoreTrait for Store {
    async fn load_v4(&self) -> io::Result<HashSet<SocketAddrV4>> {
        let file = fs::File::open(&self.path_v4).await?;
        let mut lines = BufReader::new(file).lines();

        let mut contacts = HashSet::new();

        while let Some(line) = lines.next_line().await? {
            match line.parse() {
                Ok(contact) => {
                    contacts.insert(contact);
                }
                Err(error) => {
                    tracing::warn!(
                        "dht_contacts::Store: Failed to parse IPv4 contact {:?} E:{:?}",
                        line,
                        error
                    )
                }
            }
        }

        Ok(contacts)
    }

    async fn load_v6(&self) -> io::Result<HashSet<SocketAddrV6>> {
        let file = fs::File::open(&self.path_v6).await?;
        let mut lines = BufReader::new(file).lines();

        let mut contacts = HashSet::new();

        while let Some(line) = lines.next_line().await? {
            match line.parse() {
                Ok(contact) => {
                    contacts.insert(contact);
                }
                Err(error) => {
                    tracing::warn!(
                        "dht_contacts::Store: Failed to parse IPv6 contact {:?} E:{:?}",
                        line,
                        error
                    )
                }
            }
        }

        Ok(contacts)
    }

    async fn store_v4(&self, contacts: HashSet<SocketAddrV4>) -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.path_v4)
            .await?;

        for contact in contacts {
            file.write_all(contact.to_string().as_bytes()).await?;
            file.write_all(b"\n").await?;
        }

        Ok(())
    }

    async fn store_v6(&self, contacts: HashSet<SocketAddrV6>) -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.path_v6)
            .await?;

        for contact in contacts {
            file.write_all(contact.to_string().as_bytes()).await?;
            file.write_all(b"\n").await?;
        }

        Ok(())
    }
}
