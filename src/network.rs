use crate::{
    Index,
    message_broker::MessageBroker,
    object_stream::ObjectStream,
    replica_discovery::{ReplicaDiscovery, RuntimeId},
    replica_id::ReplicaId,
    scoped_task_set::{ScopedTaskHandle, ScopedTaskSet},
};

use std::{
    collections::{HashMap, HashSet},
    io,
    net::SocketAddr,
    sync::Arc,
};

use tokio::net::{TcpListener, TcpStream};

use std::sync::Mutex;

pub struct Network {
    _tasks: ScopedTaskSet,
}

impl Network {
    pub fn new(_enable_discovery: bool, index: Index) -> io::Result<Self> {
        let tasks = ScopedTaskSet::default();
        let task_handle = tasks.handle().clone();

        let inner = Arc::new(Inner {
            this_replica_id: ReplicaId::random(),
            message_brokers: Mutex::new(HashMap::new()),
            to_forget: Mutable::new(HashSet::new()),
            task_handle,
            index,
        });

        tasks.spawn(async move {
            inner.start().await.unwrap();
        });

        Ok(Self { _tasks: tasks })
    }
}

struct Inner {
    this_replica_id: ReplicaId,
    message_brokers: Mutex<HashMap<ReplicaId, MessageBroker>>,
    to_forget: Mutable<HashSet<RuntimeId>>,
    task_handle: ScopedTaskHandle,
    index: Index,
}

impl Inner {
    async fn start(self: Arc<Self>) -> io::Result<()> {
        let any_addr = SocketAddr::from(([0, 0, 0, 0], 0));
        let listener = TcpListener::bind(any_addr).await?;

        let s1 = self.clone();
        let s2 = self.clone();

        self.task_handle
            .spawn(s1.run_discovery(listener.local_addr().unwrap().port()));
        self.task_handle.spawn(s2.run_listener(listener));

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
                        self.task_handle.spawn(async move {
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
            self.task_handle.spawn(async move {
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
        let index = self.index.clone();

        let broker = brokers.entry(their_replica_id).or_insert_with(|| {
            MessageBroker::new(
                index,
                Box::new(move || {
                    let mut brokers = s.message_brokers.lock().unwrap();
                    brokers.remove(&their_replica_id);
                    if let Some(discovery_id) = discovery_id {
                        s.to_forget.modify(|set| set.insert(discovery_id));
                    }
                }),
            )
        });

        broker.add_connection(os);

        Ok(())
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
