use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use btdht::MainlineDht;
use net::quic;
use state_monitor::StateMonitor;
use tokio::{select, sync::Notify};
use tracing::Span;

use super::{DhtContactsStoreTrait, monitored::MonitoredDht};

pub(super) struct RestartableDht {
    shared: Arc<Shared>,
}

impl RestartableDht {
    pub fn new(
        contacts: Option<Arc<dyn DhtContactsStoreTrait>>,
        parent_monitor: StateMonitor,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                contacts,
                parent_monitor,
                span: Span::current(),
                inner: Mutex::new(Inner {
                    observer_count: 0,
                    state: State::Disabled,
                    socket_maker: None,
                    routers: HashSet::new(),
                }),
                notify: Notify::new(),
            }),
        }
    }

    pub fn bind(&self, socket_maker: Option<quic::SideChannelMaker>) {
        self.restart(|inner| {
            inner.socket_maker = socket_maker;
        })
    }

    pub fn set_routers(&self, routers: HashSet<String>) {
        self.restart(|inner| {
            inner.routers = routers;
        })
    }

    pub fn routers(&self) -> HashSet<String> {
        self.shared.inner.lock().unwrap().routers.clone()
    }

    pub fn observe(&self) -> ObservableDht {
        let mut inner = self.shared.inner.lock().unwrap();

        inner.observer_count = inner
            .observer_count
            .checked_add(1)
            .expect("DHT observer count overflow");

        ObservableDht {
            shared: self.shared.clone(),
        }
    }

    fn restart<F>(&self, f: F)
    where
        F: FnOnce(&mut Inner),
    {
        let mut inner = self.shared.inner.lock().unwrap();

        f(&mut inner);

        match inner.state {
            State::Enabled(_) | State::Enabling => {
                inner.state = State::Enabling;
                self.shared.notify.notify_waiters();
            }
            State::Disabled => (),
        }
    }
}

pub(super) struct ObservableDht {
    shared: Arc<Shared>,
}

impl ObservableDht {
    pub async fn get(&self) -> MainlineDht {
        loop {
            let notified = self.shared.notify.notified();

            let (socket, routers) = {
                let inner = self.shared.inner.lock().unwrap();

                match &inner.state {
                    State::Enabled(dht) => return dht.dht.clone(),
                    State::Enabling => (None, HashSet::new()),
                    State::Disabled => {
                        if let Some(maker) = &inner.socket_maker {
                            (Some(maker.make()), inner.routers.clone())
                        } else {
                            (None, HashSet::new())
                        }
                    }
                }
            };

            if let Some(socket) = socket {
                let start = MonitoredDht::start(
                    socket,
                    routers,
                    self.shared.contacts.clone(),
                    &self.shared.parent_monitor,
                    &self.shared.span,
                );

                // When we get a notification, interrupt the startup and start again. This is
                // because the notification could have been triggered by a configuration change
                // (socket rebind or DHT routers change).
                select! {
                    dht = start => {
                        let mut inner = self.shared.inner.lock().unwrap();
                        inner.state = State::Enabled(dht);
                        self.shared.notify.notify_waiters();
                    }
                    _ = notified => (),
                };
            } else {
                notified.await;
            }
        }
    }

    pub fn try_get(&self) -> Option<MainlineDht> {
        match &self.shared.inner.lock().unwrap().state {
            State::Enabled(dht) => Some(dht.dht.clone()),
            State::Enabling | State::Disabled => None,
        }
    }
}

impl Drop for ObservableDht {
    fn drop(&mut self) {
        let mut inner = self.shared.inner.lock().unwrap();
        inner.observer_count = inner
            .observer_count
            .checked_sub(1)
            .expect("DHT observer count underflow");

        if inner.observer_count == 0 {
            // We are the last observer - destroy the dht
            inner.state = State::Disabled;
        }
    }
}

struct Shared {
    contacts: Option<Arc<dyn DhtContactsStoreTrait>>,
    parent_monitor: StateMonitor,
    span: Span,
    inner: Mutex<Inner>,
    notify: Notify,
}

struct Inner {
    observer_count: usize,
    state: State,
    socket_maker: Option<quic::SideChannelMaker>,
    routers: HashSet<String>,
}

enum State {
    Disabled,
    Enabling,
    Enabled(MonitoredDht),
}

/*

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
*/
