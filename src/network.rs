use crate::async_object::{AbortHandles, AsyncObject, AsyncObjectTrait};
use crate::message_broker::MessageBroker;
use crate::object_stream::ObjectStream;
use crate::replica_discovery::{ReplicaDiscovery, RuntimeId};
use crate::replica_id::ReplicaId;

use std::{
    collections::{HashMap, HashSet},
    io,
    net::SocketAddr,
    sync::Arc,
};

use tokio::net::{TcpListener, TcpStream};

use std::sync::Mutex;

pub struct Network {
    this_replica_id: ReplicaId,
    message_brokers: Mutex<HashMap<ReplicaId, AsyncObject<MessageBroker>>>,
    to_forget: Mutable<HashSet<RuntimeId>>,
    abort_handles: AbortHandles,
}

impl Network {
    pub fn new(_enable_discovery: bool) -> io::Result<AsyncObject<Network>> {
        let n = Arc::new(Network {
            this_replica_id: ReplicaId::random(),
            message_brokers: Mutex::new(HashMap::new()),
            to_forget: Mutable::new(HashSet::new()),
            abort_handles: AbortHandles::new(),
        });
        let n_ = n.clone();
        n.abortable_spawn(async move {
            n_.start().await.unwrap();
        });
        Ok(AsyncObject::new(n))
    }

    async fn start(self: Arc<Self>) -> io::Result<()> {
        let any_addr = SocketAddr::from(([0, 0, 0, 0], 0));
        let listener = TcpListener::bind(any_addr).await?;

        let s1 = self.clone();
        let s2 = self.clone();

        self.abortable_spawn(s1.run_discovery(listener.local_addr().unwrap().port()));
        self.abortable_spawn(s2.run_listener(listener));

        Ok(())
    }

    async fn run_discovery(self: Arc<Self>, listener_port: u16) {
        let discovery =
            ReplicaDiscovery::new(listener_port).expect("Failed to create ReplicaDiscovery");

        loop {
            tokio::select! {
                found = discovery.wait_for_activity() => {

                    for (id, addr) in found {
                        let s = self.clone();
                        self.abortable_spawn(async move {
                            let socket = match TcpStream::connect(addr).await {
                                Ok(socket) => socket,
                                Err(_) => return,
                            };
                            s.handle_new_connection(socket, Some(id)).await.ok();
                        });
                    }
                }
                _ = self.to_forget.changed() => {
                    let mut set = self.to_forget.value.lock().unwrap();
                    for id in &*set {
                        discovery.forget(id);
                    }
                    set.clear();
                }
            }
        }
    }

    async fn run_listener(self: Arc<Self>, listener: TcpListener) {
        loop {
            let (socket, _addr) = listener
                .accept()
                .await
                .expect("Failed to start TcpListener");

            let s = self.clone();
            self.abortable_spawn(async move {
                s.handle_new_connection(socket, None).await.ok();
            });
        }
    }

    async fn handle_new_connection(
        self: Arc<Self>,
        socket: TcpStream,
        discovery_id: Option<RuntimeId>,
    ) -> io::Result<()> {
        let mut os = ObjectStream::new(socket);
        os.write(&self.this_replica_id).await?;
        let their_replica_id = os.read::<ReplicaId>().await?;

        let s = self.clone();
        let mut brokers = self.message_brokers.lock().unwrap();

        let broker = brokers.entry(their_replica_id).or_insert_with(|| {
            MessageBroker::new(Box::new(move || {
                let mut brokers = s.message_brokers.lock().unwrap();
                brokers.remove(&their_replica_id);
                if let Some(discovery_id) = discovery_id {
                    s.to_forget.modify(|set| set.insert(discovery_id));
                }
            }))
        });

        broker.arc().add_connection(os);

        Ok(())
    }
}

impl AsyncObjectTrait for Network {
    fn abort_handles(&self) -> &AbortHandles {
        &self.abort_handles
    }
}

struct Mutable<T> {
    notify: tokio::sync::Notify,
    value: std::sync::Mutex<T>,
}

impl<T> Mutable<T> {
    pub fn new(value: T) -> Mutable<T> {
        Mutable {
            notify: tokio::sync::Notify::new(),
            value: std::sync::Mutex::new(value),
        }
    }

    pub fn modify<F, R>(&self, f: F)
    where
        F: FnOnce(&mut T) -> R + Sized,
    {
        let mut v = self.value.lock().unwrap();
        f(&mut v);
        self.notify.notify_one();
    }

    pub async fn changed(&self) {
        self.notify.notified().await
    }
}
