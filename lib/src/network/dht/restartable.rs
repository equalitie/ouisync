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

// Wrapper for a DHT instance that can be stopped and restarted at any point.
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
                    state: State::Stopped,
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

        inner.state = State::Stopped;
        self.shared.notify.notify_waiters();
    }
}

pub(super) struct ObservableDht {
    shared: Arc<Shared>,
}

impl ObservableDht {
    /// Waits until the DHT has been started (returns `Some`) or disabled (returns `None`).
    pub async fn started_or_disabled(&self) -> Option<MainlineDht> {
        loop {
            let notified = self.shared.notify.notified();

            let (socket, routers) = {
                let mut inner = self.shared.inner.lock().unwrap();

                match &inner.state {
                    State::Stopped => {
                        if let Some(socket) = inner.socket_maker.as_ref().map(|m| m.make()) {
                            inner.state = State::Starting;
                            (Some(socket), inner.routers.clone())
                        } else {
                            return None;
                        }
                    }
                    State::Started(dht) => return Some(dht.dht.clone()),
                    State::Starting => (None, HashSet::new()),
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
                        inner.state = State::Started(dht);
                        self.shared.notify.notify_waiters();
                    }
                    _ = notified => (),
                };
            } else {
                notified.await;
            }
        }
    }

    /// Waits until the DHT becomes enabled. Note: calling `started_or_diabled` afterwards might
    /// still return `None` if the DHT has been disabled in the meantime.
    pub async fn enabled(&self) {
        loop {
            let notified = self.shared.notify.notified();

            if self.shared.inner.lock().unwrap().socket_maker.is_some() {
                break;
            }

            notified.await;
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
            inner.state = State::Stopped;
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
    Stopped,
    Starting,
    Started(MonitoredDht),
}
