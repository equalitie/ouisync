use std::{collections::HashSet, future::pending, io, net::SocketAddr, sync::Arc, time::Duration};

use async_trait::async_trait;
use btdht::MainlineDht;
use net::{quic, udp::DatagramSocket};
use scoped_task::ScopedJoinHandle;
use state_monitor::StateMonitor;
use tokio::time;
use tracing::{Instrument, Span};

use super::DhtContactsStoreTrait;

// Wrapper for a DHT instance that periodically outputs it's state to the provided StateMonitor.
pub(super) struct MonitoredDht {
    pub(super) dht: MainlineDht,
    _monitoring_task: ScopedJoinHandle<()>,
    _periodic_dht_node_load_task: Option<ScopedJoinHandle<()>>,
}

impl MonitoredDht {
    pub async fn start(
        socket: quic::SideChannel,
        routers: HashSet<String>,
        contacts: Option<Arc<dyn DhtContactsStoreTrait>>,
        parent_monitor: &StateMonitor,
        span: &Span,
    ) -> Self {
        // TODO: Unwrap
        let local_addr = socket.local_addr().unwrap();

        let (is_v4, monitor_name, span) = match local_addr {
            SocketAddr::V4(_) => (true, "IPv4", tracing::info_span!(parent: span, "DHT/IPv4")),
            SocketAddr::V6(_) => (false, "IPv6", tracing::info_span!(parent: span, "DHT/IPv6")),
        };

        let monitor = parent_monitor.make_child(monitor_name);

        // TODO: load the DHT state from a previous save if it exists.
        let mut builder = MainlineDht::builder()
            .add_routers(routers)
            .set_read_only(false);

        if let Some(contacts) = &contacts {
            let initial_contacts = Self::load_initial_contacts(is_v4, &**contacts).await;

            for contact in initial_contacts {
                builder = builder.add_node(contact);
            }
        }

        let dht = span
            .in_scope(|| builder.start(Socket(socket)))
            // TODO: `start` only fails if the socket has been closed. That shouldn't be the case
            // there but better check.
            .unwrap();

        // Spawn a task to monitor the DHT status.
        let monitoring_task = {
            let dht = dht.clone();

            let first_bootstrap = monitor.make_value("first_bootstrap", "in progress");
            let probe_counter = monitor.make_value("probe_counter", 0);
            let is_running = monitor.make_value("is_running", false);
            let bootstrapped = monitor.make_value("bootstrapped", false);
            let good_nodes = monitor.make_value("good_nodes", 0);
            let questionable_nodes = monitor.make_value("questionable_nodes", 0);
            let buckets = monitor.make_value("buckets", 0);

            async move {
                tracing::info!("bootstrap started");

                if dht.bootstrapped().await {
                    *first_bootstrap.get() = "done";
                    tracing::info!("bootstrap complete");
                } else {
                    *first_bootstrap.get() = "failed";
                    tracing::error!("bootstrap failed");

                    // Don't `return`, instead halt here so that the `first_bootstrap` monitored
                    // value is preserved for the user to see.
                    pending::<()>().await;
                }

                loop {
                    *probe_counter.get() += 1;

                    if let Some(state) = dht.get_state().await {
                        *is_running.get() = true;
                        *bootstrapped.get() = true;
                        *good_nodes.get() = state.good_node_count;
                        *questionable_nodes.get() = state.questionable_node_count;
                        *buckets.get() = state.bucket_count;
                    } else {
                        *is_running.get() = false;
                        *bootstrapped.get() = false;
                        *good_nodes.get() = 0;
                        *questionable_nodes.get() = 0;
                        *buckets.get() = 0;
                    }

                    time::sleep(Duration::from_secs(5)).await;
                }
            }
        };
        let monitoring_task = monitoring_task.instrument(span.clone());
        let monitoring_task = scoped_task::spawn(monitoring_task);

        let _periodic_dht_node_load_task = contacts.map(|contacts| {
            scoped_task::spawn(
                Self::keep_reading_contacts(is_v4, dht.clone(), contacts).instrument(span),
            )
        });

        Self {
            dht,
            _monitoring_task: monitoring_task,
            _periodic_dht_node_load_task,
        }
    }

    /// Periodically read contacts from the `dht` and send it to `on_periodic_dht_node_load_tx`.
    async fn keep_reading_contacts(
        is_v4: bool,
        dht: MainlineDht,
        contacts_store: Arc<dyn DhtContactsStoreTrait>,
    ) {
        let mut reported_failure = false;

        // Give `MainlineDht` a chance to bootstrap.
        time::sleep(Duration::from_secs(10)).await;

        loop {
            let (good, questionable) = match dht.load_contacts().await {
                Ok((good, questionable)) => (good, questionable),
                Err(error) => {
                    tracing::warn!("DhtDiscovery stopped reading contacts: {error:?}");
                    break;
                }
            };

            // TODO: Make use of the information which is good and which questionable.
            let mix = good.union(&questionable);

            if is_v4 {
                let mix = mix.filter_map(|addr| match addr {
                    SocketAddr::V4(addr) => Some(*addr),
                    SocketAddr::V6(_) => None,
                });

                match contacts_store.store_v4(mix.collect()).await {
                    Ok(()) => reported_failure = false,
                    Err(error) => {
                        if !reported_failure {
                            reported_failure = true;
                            tracing::error!("DhtDiscovery failed to write contacts {error:?}");
                        }
                    }
                }
            } else {
                let mix = mix.filter_map(|addr| match addr {
                    SocketAddr::V4(_) => None,
                    SocketAddr::V6(addr) => Some(*addr),
                });

                match contacts_store.store_v6(mix.collect()).await {
                    Ok(()) => reported_failure = false,
                    Err(error) => {
                        if !reported_failure {
                            reported_failure = true;
                            tracing::error!("DhtDiscovery failed to write contacts {error:?}");
                        }
                    }
                }
            }

            time::sleep(Duration::from_secs(60)).await;
        }
    }

    async fn load_initial_contacts(
        is_v4: bool,
        contacts_store: &(impl DhtContactsStoreTrait + ?Sized),
    ) -> HashSet<SocketAddr> {
        if is_v4 {
            match contacts_store.load_v4().await {
                Ok(contacts) => contacts.iter().cloned().map(SocketAddr::V4).collect(),
                Err(error) => {
                    tracing::error!("Failed to load DHT IPv4 contacts {:?}", error);
                    Default::default()
                }
            }
        } else {
            match contacts_store.load_v6().await {
                Ok(contacts) => contacts.iter().cloned().map(SocketAddr::V6).collect(),
                Err(error) => {
                    tracing::error!("Failed to load DHT IPv4 contacts {:?}", error);
                    Default::default()
                }
            }
        }
    }
}

struct Socket(quic::SideChannel);

#[async_trait]
impl btdht::SocketTrait for Socket {
    async fn send_to(&self, buf: &[u8], target: &SocketAddr) -> io::Result<()> {
        self.0.send_to(buf, *target).await?;
        Ok(())
    }

    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.0.recv_from(buf).await
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.local_addr()
    }
}
