use crate::async_object::{AbortHandles, AsyncObject, AsyncObjectTrait};
use crate::message_broker::MessageBroker;
use crate::object_stream::ObjectStream;
use crate::replica_discovery::ReplicaDiscovery;
use crate::replica_id::ReplicaId;

use std::{collections::HashMap, io, net::SocketAddr, sync::Arc};

use tokio::{
    net::{TcpListener, TcpStream},
    sync::Mutex,
};

pub struct Network {
    this_replica_id: ReplicaId,
    message_brokers: Mutex<HashMap<ReplicaId, AsyncObject<MessageBroker>>>,
    abort_handles: AbortHandles,
}

impl Network {
    pub fn new(_enable_discovery: bool) -> io::Result<AsyncObject<Network>> {
        let n = Arc::new(Network {
            this_replica_id: ReplicaId::random(),
            message_brokers: Mutex::new(HashMap::new()),
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
            let found = discovery.wait_for_activity().await;

            for addr in found {
                let s = self.clone();
                self.abortable_spawn(async move {
                    let socket = match TcpStream::connect(addr).await {
                        Ok(socket) => socket,
                        Err(_) => return,
                    };
                    s.handle_new_connection(socket).await.ok();
                });
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
                s.handle_new_connection(socket).await.ok();
            });
        }
    }

    async fn handle_new_connection(self: Arc<Self>, socket: TcpStream) -> io::Result<()> {
        let mut os = ObjectStream::new(socket);
        os.write(&self.this_replica_id).await?;
        let their_replica_id = os.read::<ReplicaId>().await?;

        let mut brokers = self.message_brokers.lock().await;

        let broker = brokers
            .entry(their_replica_id)
            .or_insert_with(|| MessageBroker::new());

        broker.arc().add_connection(os);

        Ok(())
    }
}

impl AsyncObjectTrait for Network {
    fn abort_handles(&self) -> &AbortHandles {
        &self.abort_handles
    }
}
