use std::{
    collections::HashSet,
    sync::{Arc, Weak},
};

use net::quic;
use state_monitor::StateMonitor;
use tracing::Span;

use super::{DhtContactsStoreTrait, TaskOrResult, monitored::MonitoredDht};

// Wrapper for a DHT instance that can be stopped and restarted at any point.
pub(super) struct RestartableDht {
    socket_maker: Option<quic::SideChannelMaker>,
    dht: Weak<TaskOrResult<MonitoredDht>>,
    routers: HashSet<String>,
    contacts: Option<Arc<dyn DhtContactsStoreTrait>>,
}

impl RestartableDht {
    pub fn new(
        socket_maker: Option<quic::SideChannelMaker>,
        contacts: Option<Arc<dyn DhtContactsStoreTrait>>,
    ) -> Self {
        Self {
            socket_maker,
            dht: Weak::new(),
            routers: HashSet::new(),
            contacts,
        }
    }

    // Retrieve a shared pointer to a running DHT instance if there is one already or start a new
    // one. When all such pointers are dropped, the underlying DHT is terminated.
    pub fn fetch(
        &mut self,
        monitor: &StateMonitor,
        span: &Span,
    ) -> Option<Arc<TaskOrResult<MonitoredDht>>> {
        if let Some(dht) = self.dht.upgrade() {
            Some(dht)
        } else if let Some(maker) = &self.socket_maker {
            let socket = maker.make();
            let dht = MonitoredDht::start(
                socket,
                self.routers.clone(),
                self.contacts.clone(),
                monitor,
                span,
            );
            let dht = Arc::new(dht);

            self.dht = Arc::downgrade(&dht);

            Some(dht)
        } else {
            None
        }
    }

    pub fn rebind(&mut self, socket_maker: Option<quic::SideChannelMaker>) {
        self.socket_maker = socket_maker;
        self.dht = Weak::new();
    }

    pub fn set_routers(&mut self, routers: HashSet<String>) {
        self.routers = routers;
        self.dht = Weak::new();
    }

    pub fn routers(&self) -> &HashSet<String> {
        &self.routers
    }
}
