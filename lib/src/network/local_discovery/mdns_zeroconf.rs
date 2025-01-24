use super::mdns_common;
use crate::network::{
    peer_addr::{PeerAddr, PeerPort},
    seen_peers::SeenPeer,
};
use scoped_task;
use std::{
    any::Any,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex, Weak},
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::{select, sync::mpsc, task, time::sleep};
use zeroconf::{
    prelude::*, BrowserEvent, MdnsBrowser, MdnsService, ServiceDiscovery, ServiceRegistration,
    ServiceType,
};

pub struct LocalDiscovery {
    peer_rx: mpsc::UnboundedReceiver<SeenPeer>,
    watcher_finished: Arc<Flag>,
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

        let watcher_finished = Arc::new(Flag::new());

        let _service_watcher = scoped_task::spawn({
            let watcher_finished = watcher_finished.clone();

            async move {
                loop {
                    let service_finished = watcher_finished.child();

                    tracing::debug!("Starting service");

                    let publish_finished = start_publishing_service_thread(
                        service_name.clone(),
                        service_type.clone(),
                        listener_port.number(),
                        service_finished.clone(),
                    );

                    let browser_finished = start_browser_service_thread(
                        service_name.clone(),
                        service_type.clone(),
                        peer_tx.clone(),
                        service_finished.clone(),
                    );

                    publish_finished.await.unwrap_or(());
                    browser_finished.await.unwrap_or(());

                    tracing::debug!("Service stopped");

                    select! {
                        // TODO: Exponential backoff?
                        _ = sleep(Duration::from_secs(5)) => {},
                        _ = watcher_finished.becomes_true() => {
                            break;
                        }
                    }
                }

                tracing::debug!("Service finished (won't restart)");
            }
        });

        Self {
            peer_rx,
            watcher_finished,
            _service_watcher,
        }
    }

    pub async fn recv(&mut self) -> Option<SeenPeer> {
        self.peer_rx.recv().await
    }
}

impl Drop for LocalDiscovery {
    fn drop(&mut self) {
        self.watcher_finished.mark_true();
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
        service.set_registered_callback(Box::new(on_publish_event));
        service.set_context(Box::new(Arc::new(BeaconContext {
            finished: finished.clone(),
        })));

        let event_loop = match service.register() {
            Ok(event_loop) => event_loop,
            Err(error) => {
                if !finished.mark_true() {
                    tracing::error!("Service/Publish failed to register {error:?}");
                }
                return;
            }
        };

        loop {
            // calling `poll()` will keep this service alive
            if let Err(error) = event_loop.poll(Duration::from_secs(1)) {
                if !finished.mark_true() {
                    tracing::warn!("Service/Publish stopped with error {error:?}");
                }
            }
            if finished.is_true() {
                tracing::debug!("Service/Publish finished");
                break;
            }
        }
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

        browser.set_service_callback(Box::new(on_browser_event));
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
                    tracing::error!("Service/Browse failed to register {error:?}");
                }
                return;
            }
        };

        loop {
            // calling `poll()` will keep this browser alive
            if let Err(error) = event_loop.poll(Duration::from_secs(1)) {
                if !finished.mark_true() {
                    tracing::warn!("Service/Browse stopped with error {error:?}");
                }
            }
            if finished.is_true() {
                tracing::debug!("Service/Browse finished");
                break;
            }
        }
    })
}

#[derive(Clone)]
struct Flag {
    parent: Option<Weak<Flag>>,
    flag: Arc<Mutex<bool>>,
    changed: Changed,
}

impl Flag {
    fn new() -> Self {
        Self {
            parent: None,
            flag: Arc::new(Mutex::new(false)),
            changed: Changed::new(),
        }
    }

    fn child(self: &Arc<Self>) -> Self {
        Self {
            parent: Some(Arc::downgrade(self)),
            flag: Arc::new(Mutex::new(false)),
            changed: Changed::new(),
        }
    }

    // Returns previous value
    fn mark_true(&self) -> bool {
        let mut lock = self.flag.lock().unwrap();
        let prev = *lock;
        *lock = true;
        self.changed.mark_changed();
        return prev;
    }

    fn is_true(&self) -> bool {
        if *self.flag.lock().unwrap() {
            return true;
        }
        let Some(parent) = &self.parent else {
            // This is the root.
            return false;
        };
        // Check parents recursively
        match parent.upgrade() {
            Some(parent) => parent.is_true(),
            // Parent got destroyed
            None => true,
        }
    }

    async fn becomes_true(&self) {
        let mut up_to_parent = Vec::new();

        up_to_parent.push(self.changed.clone());

        let mut current_opt = self.parent.clone();

        while let Some(current) = &current_opt {
            let Some(current) = current.upgrade() else {
                return;
            };
            up_to_parent.push(current.changed.clone());
            current_opt = current.parent.clone();
        }

        futures_util::future::select_all(up_to_parent.iter()).await;
    }
}

#[derive(Clone)]
struct Changed {
    inner: Arc<Mutex<ChangedInner>>,
}

impl Changed {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ChangedInner {
                changed: false,
                wakers: Default::default(),
            })),
        }
    }

    fn mark_changed(&self) {
        self.inner.lock().unwrap().mark_changed();
    }
}

struct ChangedInner {
    changed: bool,
    wakers: Vec<Waker>,
}

impl ChangedInner {
    fn mark_changed(&mut self) {
        self.changed = true;
        for waker in self.wakers.drain(..) {
            waker.wake();
        }
    }
}

impl Drop for ChangedInner {
    fn drop(&mut self) {
        self.mark_changed();
    }
}

impl Future for &Changed {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut inner = self.inner.lock().unwrap();
        if inner.changed {
            return Poll::Ready(());
        }
        inner.wakers.push(cx.waker().clone());
        Poll::Pending
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

fn on_publish_event(result: zeroconf::Result<ServiceRegistration>, context: Option<Arc<dyn Any>>) {
    let context = context
        .as_ref()
        .expect("could not get context")
        .downcast_ref::<Arc<BeaconContext>>()
        .expect("error down-casting beacon context");

    match result {
        Err(error) => {
            if !context.finished.mark_true() {
                tracing::error!("Service/Publish failed to register: {error:?}");
            }
        }
        Ok(_) => {
            tracing::debug!("Service/Publish registered successfully");
        }
    }
}

fn on_browser_event(result: zeroconf::Result<BrowserEvent>, context: Option<Arc<dyn Any>>) {
    let context = context
        .as_ref()
        .expect("could not get context")
        .downcast_ref::<Arc<DiscoveryContext>>()
        .expect("error down-casting discovery context");

    let event = match result {
        Ok(event) => event,
        Err(err) => {
            // TODO: We get serious and non-serious errors here and we should only finish this
            // service on the serious ones. The serious one that I know of is when the daemon stops
            // and a non serious one is when a service is non-resolvable (e.g. because a replica
            // shuts down but its service name is still in some cache).
            if !context.finished.mark_true() {
                tracing::debug!("Service/Browse discover error: {:?}", err);
            }
            return;
        }
    };

    match event {
        BrowserEvent::Add(service) => {
            if service.name() == &context.this_service_name {
                tracing::trace!("Service/Browse ignoring same instance name");
                return;
            }

            let peer_addr = match parse_peer_addr(&service) {
                Ok(peer_addr) => peer_addr,
                Err(reason) => {
                    tracing::debug!("Service/Browse failed to parse peer address: {reason}");
                    return;
                }
            };

            if let Some(seen_peer) = context
                .seen_peers
                .lock()
                .unwrap()
                .insert(service.name().into(), peer_addr)
            {
                tracing::debug!(
                    "Service/Browse discovered: {:?}:{:?}",
                    service.name(),
                    peer_addr
                );
                context.peer_tx.send(seen_peer).unwrap_or(());
            }
        }
        BrowserEvent::Remove(service) => {
            context
                .seen_peers
                .lock()
                .unwrap()
                .remove(service.name().clone());
        }
    }
}

fn parse_peer_addr(service: &ServiceDiscovery) -> Result<PeerAddr, String> {
    let ip_addr = match service.address().parse() {
        Ok(ip_addr) => ip_addr,
        Err(_) => {
            return Err(format!("Failed to parse address {:?}", service.address()));
        }
    };

    let sock_addr = SocketAddr::new(ip_addr, *service.port());

    match service.service_type().protocol().as_ref() {
        "tcp" => Ok(PeerAddr::Tcp(sock_addr)),
        "udp" => Ok(PeerAddr::Quic(sock_addr)),
        proto => {
            return Err(format!("Invalid protocol {proto:?}"));
        }
    }
}
