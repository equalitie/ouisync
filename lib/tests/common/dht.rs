use std::{
    collections::HashSet,
    io,
    net::{SocketAddr, SocketAddrV4, SocketAddrV6},
    sync::Mutex,
};

use async_trait::async_trait;
use ouisync::DhtContactsStoreTrait;

pub(crate) struct TestDhtContacts {
    v4: Mutex<HashSet<SocketAddrV4>>,
    v6: Mutex<HashSet<SocketAddrV6>>,
}

impl TestDhtContacts {
    pub fn new(contacts: impl IntoIterator<Item = SocketAddr>) -> Self {
        let mut v4 = HashSet::new();
        let mut v6 = HashSet::new();

        for addr in contacts {
            match addr {
                SocketAddr::V4(addr) => v4.insert(addr),
                SocketAddr::V6(addr) => v6.insert(addr),
            };
        }

        Self {
            v4: Mutex::new(v4),
            v6: Mutex::new(v6),
        }
    }
}

#[async_trait]
impl DhtContactsStoreTrait for TestDhtContacts {
    async fn load_v4(&self) -> io::Result<HashSet<SocketAddrV4>> {
        Ok(self.v4.lock().unwrap().clone())
    }

    async fn load_v6(&self) -> io::Result<HashSet<SocketAddrV6>> {
        Ok(self.v6.lock().unwrap().clone())
    }

    async fn store_v4(&self, contacts: HashSet<SocketAddrV4>) -> io::Result<()> {
        *self.v4.lock().unwrap() = contacts;
        Ok(())
    }

    async fn store_v6(&self, contacts: HashSet<SocketAddrV6>) -> io::Result<()> {
        *self.v6.lock().unwrap() = contacts;
        Ok(())
    }
}
