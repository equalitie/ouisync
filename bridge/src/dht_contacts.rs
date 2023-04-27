use async_trait::async_trait;
use indexmap::set::IndexSet;
use ouisync_lib::{deadlock::AsyncMutex, network::dht_discovery::DhtContactsStoreTrait};
use std::{
    cmp::Eq,
    collections::HashSet,
    fmt::{Debug, Display},
    hash::Hash,
    io,
    marker::PhantomData,
    net::{SocketAddrV4, SocketAddrV6},
    path::{Path, PathBuf},
    str::FromStr,
};
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
};

const MAX_STORE_SIZE: usize = 2000;

pub struct Store {
    store_v4: AsyncMutex<StoreT<SocketAddrV4>>,
    store_v6: AsyncMutex<StoreT<SocketAddrV6>>,
}

impl Store {
    pub fn new<P: AsRef<Path>>(store_dir: P) -> Self {
        Self {
            store_v4: AsyncMutex::new(StoreT::new(store_dir.as_ref().join("dht_contacts_v4.txt"))),
            store_v6: AsyncMutex::new(StoreT::new(store_dir.as_ref().join("dht_contacts_v6.txt"))),
        }
    }
}

#[async_trait]
impl DhtContactsStoreTrait for Store {
    async fn load_v4(&self) -> io::Result<HashSet<SocketAddrV4>> {
        self.store_v4.lock().await.load().await
    }

    async fn load_v6(&self) -> io::Result<HashSet<SocketAddrV6>> {
        self.store_v6.lock().await.load().await
    }

    async fn store_v4(&self, contacts: HashSet<SocketAddrV4>) -> io::Result<()> {
        self.store_v4.lock().await.store(contacts).await
    }

    async fn store_v6(&self, contacts: HashSet<SocketAddrV6>) -> io::Result<()> {
        self.store_v6.lock().await.store(contacts).await
    }
}

struct StoreT<Addr> {
    path: PathBuf,
    cache: IndexSet<Addr>,
    _phantom: PhantomData<Addr>,
}

impl<Addr> StoreT<Addr>
where
    Addr: AddrExt + FromStr + Hash + Eq + Debug + Display + Clone + Copy,
    <Addr as FromStr>::Err: Debug,
{
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            cache: Default::default(),
            _phantom: PhantomData,
        }
    }

    async fn load(&mut self) -> io::Result<HashSet<Addr>> {
        let file = fs::File::open(&self.path).await?;
        let mut lines = BufReader::new(file).lines();

        while let Some(line) = lines.next_line().await? {
            match line.parse() {
                Ok(contact) => {
                    self.cache.insert(contact);
                }
                Err(error) => {
                    tracing::warn!(
                        "dht_contacts::Store: Failed to parse {} contact {:?} E:{:?}",
                        Addr::version_str(),
                        line,
                        error
                    )
                }
            }
        }

        Ok(self.cache.iter().cloned().collect())
    }

    async fn store(&mut self, contacts: HashSet<Addr>) -> io::Result<()> {
        if !self.update_cache(&contacts) {
            return Ok(());
        }

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.path)
            .await?;

        for contact in &self.cache {
            file.write_all(contact.to_string().as_bytes()).await?;
            file.write_all(b"\n").await?;
        }

        Ok(())
    }

    // Returns true if `self.cache` was modified.
    fn update_cache(&mut self, contacts: &HashSet<Addr>) -> bool {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        let mut modified = false;

        for contact in contacts {
            modified |= self.cache.insert(*contact);

            if self.cache.len() > MAX_STORE_SIZE {
                let to_remove = rng.gen_range(0..self.cache.len());
                self.cache.swap_remove_index(to_remove);
            }
        }

        modified
    }
}

trait AddrExt {
    fn version_str() -> &'static str;
}

impl AddrExt for SocketAddrV4 {
    fn version_str() -> &'static str {
        "IPv4"
    }
}

impl AddrExt for SocketAddrV6 {
    fn version_str() -> &'static str {
        "IPv6"
    }
}
