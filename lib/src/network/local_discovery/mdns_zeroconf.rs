use super::mdns_common;
use crate::network::{
    peer_addr::{PeerAddr, PeerPort},
    seen_peers::SeenPeer,
};
use scoped_task;
use std::{
    any::Any,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::mpsc, task, time::sleep};
use zeroconf::{
    prelude::*, BrowserEvent, MdnsBrowser, MdnsService, ServiceDiscovery, ServiceRegistration,
    ServiceType,
};

pub struct LocalDiscovery {
    peer_rx: mpsc::UnboundedReceiver<SeenPeer>,
    finished: Flag,
    _service_watcher: scoped_task::ScopedJoinHandle<()>,
}

impl LocalDiscovery {
    pub fn new(listener_port: PeerPort) -> Self {
        // Unwraps are OK because the input to Service::new is hardcoded.
        let service_type = match listener_port {
            PeerPort::Tcp(_) => ServiceType::new("ouisync", "tcp").unwrap(),
            PeerPort::Quic(_) => ServiceType::new("ouisync", "udp").unwrap(),
        };

        let service_name = mdns_common::generate_instance_name();

        tracing::debug!("Service name of this replica: {service_name:?}");

        let (peer_tx, peer_rx) = mpsc::unbounded_channel();

        let _service_watcher = scoped_task::spawn(async move {
            loop {
                let finished = Flag::new();

                tracing::debug!("Starting service");

                let publish_finished = start_publishing_service_thread(
                    service_name.clone(),
                    service_type.clone(),
                    listener_port.number(),
                    finished.clone(),
                );

                let browser_finished = start_browser_service_thread(
                    service_name.clone(),
                    service_type.clone(),
                    peer_tx.clone(),
                    finished.clone(),
                );

                publish_finished.await.unwrap_or(());
                browser_finished.await.unwrap_or(());

                tracing::debug!("Service stopped");

                sleep(Duration::from_secs(3)).await;
            }
        });

        Self {
            peer_rx,
            finished: Flag::new(), // FIXME
            _service_watcher,
        }
    }

    pub async fn recv(&mut self) -> Option<SeenPeer> {
        self.peer_rx.recv().await
    }
}

impl Drop for LocalDiscovery {
    fn drop(&mut self) {
        self.finished.mark_true();
    }
}

// This thread tells Zeroconf about our service so that others can find us.
fn start_publishing_service_thread(
    service_name: String,
    service_type: ServiceType,
    listener_port: u16,
    finished: Flag,
) -> task::JoinHandle<()> {
    task::spawn_blocking(move || {
        let mut service = MdnsService::new(service_type, listener_port);

        service.set_name(&service_name);
        service.set_registered_callback(Box::new(on_service_registered));
        service.set_context(Box::new(Arc::new(BeaconContext {
            finished: finished.clone(),
        })));

        let event_loop = match service.register() {
            Ok(event_loop) => event_loop,
            Err(error) => {
                if !finished.mark_true() {
                    tracing::error!("Failed to register beacon service {error:?}");
                }
                return;
            }
        };

        loop {
            // calling `poll()` will keep this service alive
            if let Err(error) = event_loop.poll(Duration::from_secs(1)) {
                if !finished.mark_true() {
                    tracing::warn!("Beacon stopped with error {error:?}");
                }
            }
            if finished.is_true() {
                break;
            }
        }

        tracing::debug!("Beacon service finished");
    })
}

fn start_browser_service_thread(
    this_service_name: String,
    service_type: ServiceType,
    peer_tx: mpsc::UnboundedSender<SeenPeer>,
    finished: Flag,
) -> task::JoinHandle<()> {
    task::spawn_blocking(move || {
        let mut browser = MdnsBrowser::new(service_type);

        browser.set_service_callback(Box::new(on_service_discovered));
        browser.set_context(Box::new(Arc::new(DiscoveryContext {
            this_service_name,
            peer_tx,
            seen_peers: Mutex::new(mdns_common::SeenMdnsPeers::new()),
            finished: finished.clone(),
        })));

        let event_loop = match browser.browse_services() {
            Ok(event_loop) => event_loop,
            Err(error) => {
                if !finished.mark_true() {
                    tracing::error!("Failed to register browser service {error:?}");
                }
                return;
            }
        };

        loop {
            // calling `poll()` will keep this browser alive
            if let Err(error) = event_loop.poll(Duration::from_secs(1)) {
                if !finished.mark_true() {
                    tracing::warn!("Discovery stopped with error {error:?}");
                }
            }
            if finished.is_true() {
                break;
            }
        }

        tracing::debug!("Browser service finished");
    })
}

#[derive(Clone)]
struct Flag {
    flag: Arc<Mutex<bool>>,
}

impl Flag {
    fn new() -> Self {
        Self {
            flag: Arc::new(Mutex::new(false)),
        }
    }

    // Returns previous value
    fn mark_true(&self) -> bool {
        let mut lock = self.flag.lock().unwrap();
        let prev = *lock;
        *lock = true;
        return prev;
    }

    fn is_true(&self) -> bool {
        *self.flag.lock().unwrap()
    }
}

struct BeaconContext {
    finished: Flag,
}

struct DiscoveryContext {
    this_service_name: String,
    peer_tx: mpsc::UnboundedSender<SeenPeer>,
    seen_peers: Mutex<mdns_common::SeenMdnsPeers>,
    finished: Flag,
}

fn on_service_registered(
    result: zeroconf::Result<ServiceRegistration>,
    context: Option<Arc<dyn Any>>,
) {
    let context = context
        .as_ref()
        .expect("could not get context")
        .downcast_ref::<Arc<BeaconContext>>()
        .expect("error down-casting beacon context");

    match result {
        Err(error) => {
            if !context.finished.mark_true() {
                tracing::error!("Service failed to register: {error:?}");
            }
        }
        Ok(_) => {
            tracing::debug!("Service registered successfully");
        }
    }

    //context.on_service_init_tx.send(()).unwrap_or(());
}

fn on_service_discovered(result: zeroconf::Result<BrowserEvent>, context: Option<Arc<dyn Any>>) {
    let context = context
        .as_ref()
        .expect("could not get context")
        .downcast_ref::<Arc<DiscoveryContext>>()
        .expect("error down-casting discovery context");

    match result {
        Ok(BrowserEvent::Add(service)) => {
            if service.name() == &context.this_service_name {
                return;
            }

            let Some(peer_addr) = parse_peer_addr(&service) else {
                return;
            };

            if let Some(seen_peer) = context
                .seen_peers
                .lock()
                .unwrap()
                .insert(service.name().into(), peer_addr)
            {
                tracing::debug!("Service discovered: {:?}:{:?}", service.name(), peer_addr);
                context.peer_tx.send(seen_peer).unwrap_or(());
            }
        }
        Ok(BrowserEvent::Remove(service)) => {
            context
                .seen_peers
                .lock()
                .unwrap()
                .remove(service.name().clone());
        }
        Err(err) => {
            // The error only contains a string so impractical to distinguis between serious errors
            // and those that only tell us that some replica can't be resolved (e.g. because it's
            // no longer online).
            if !context.finished.mark_true() {
                tracing::debug!("Service discover error: {:?}", err);
            }
        }
    }
}

fn parse_peer_addr(service: &ServiceDiscovery) -> Option<PeerAddr> {
    let ip_addr = match service.address().parse() {
        Ok(ip_addr) => ip_addr,
        Err(_) => {
            tracing::warn!("Failed to parse address {:?}", service.address());
            return None;
        }
    };

    let sock_addr = SocketAddr::new(ip_addr, *service.port());

    match service.service_type().protocol().as_ref() {
        "tcp" => Some(PeerAddr::Tcp(sock_addr)),
        "udp" => Some(PeerAddr::Quic(sock_addr)),
        proto => {
            tracing::warn!("Invalid protocol {proto:?}");
            return None;
        }
    }
}
