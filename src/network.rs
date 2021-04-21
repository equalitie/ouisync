use crate::async_object::{AbortHandles, AsyncObject, AsyncObjectTrait};
use crate::object_stream::ObjectStream;
use crate::replica_discovery::ReplicaDiscovery;
use crate::replica_id::ReplicaId;

use std::{collections::HashMap, fmt, io, net::SocketAddr, sync::Arc};

use tokio::{
    net::{TcpListener, TcpStream},
    sync::Mutex,
};

pub struct Network {
    this_replica_id: ReplicaId,
    replicas: Mutex<HashMap<ReplicaId, Replica>>,
    abort_handles: AbortHandles,
}

impl Network {
    pub fn new(_enable_discovery: bool) -> io::Result<AsyncObject<Network>> {
        let n = Arc::new(Network {
            this_replica_id: ReplicaId::random(),
            replicas: Mutex::new(HashMap::new()),
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

    async fn run_discovery(self: Arc<Self>, listener_port: u16) -> io::Result<()> {
        let discovery = ReplicaDiscovery::new(listener_port)?;

        loop {
            let found = discovery.wait_for_activity().await;

            for addr in found {
                let s = self.clone();
                self.abortable_spawn(async move {
                    let socket = match TcpStream::connect(addr).await {
                        Ok(socket) => socket,
                        Err(_) => return,
                    };
                    s.handle_new_connection(ConnectionType::Connected, socket)
                        .await
                        .ok();
                });
            }
        }
    }

    async fn run_listener(self: Arc<Self>, listener: TcpListener) -> io::Result<()> {
        loop {
            let (socket, _addr) = listener.accept().await?;
            let s = self.clone();
            self.abortable_spawn(async move {
                s.handle_new_connection(ConnectionType::Accepted, socket)
                    .await
                    .ok();
            });
        }
    }

    async fn handle_new_connection(
        self: Arc<Self>,
        con_type: ConnectionType,
        socket: TcpStream,
    ) -> io::Result<()> {
        let mut os = ObjectStream::new(socket);
        os.write(&self.this_replica_id).await?;
        let their_replica_id = os.read::<ReplicaId>().await?;

        let mut replicas = self.replicas.lock().await;

        let replica = replicas.entry(their_replica_id).or_insert(Replica::new());

        match con_type {
            ConnectionType::Accepted => {
                replica.accepted_stream = Some(os);
            }
            ConnectionType::Connected => {
                replica.connected_stream = Some(os);
            }
        }

        println!("{:?}", replica);
        Ok(())
    }
}

impl AsyncObjectTrait for Network {
    fn abort_handles(&self) -> &AbortHandles {
        &self.abort_handles
    }
}

struct Replica {
    accepted_stream: Option<ObjectStream>,
    connected_stream: Option<ObjectStream>,
}

impl Replica {
    fn new() -> Replica {
        Replica {
            accepted_stream: None,
            connected_stream: None,
        }
    }
}

impl fmt::Debug for Replica {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut c = 0;
        if self.accepted_stream.is_some() {
            c += 1;
        }
        if self.connected_stream.is_some() {
            c += 1;
        }
        write!(f, "Replica:{:?}", c)
    }
}

#[derive(Debug)]
enum ConnectionType {
    Accepted,
    Connected,
}
