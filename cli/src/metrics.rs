use crate::{
    geo_ip::{CountryCode, GeoIp},
    state::State,
};
use anyhow::Result;
use hyper::{
    server::{conn::AddrIncoming, Server},
    service::{make_service_fn, service_fn},
    Body, Response,
};
use hyper_rustls::TlsAcceptor;
use metrics::{Gauge, Key, KeyName, Label, Level, Metadata, Recorder, Unit};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusRecorder};
use ouisync_bridge::config::{ConfigError, ConfigKey};
use ouisync_lib::{network::PeerState, PeerInfoCollector, PublicRuntimeId};
use scoped_task::ScopedAbortHandle;
use std::{
    collections::HashMap,
    convert::Infallible,
    io,
    net::SocketAddr,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::task;

const BIND_METRICS_KEY: ConfigKey<SocketAddr> =
    ConfigKey::new("bind_metrics", "Addresses to bind the metrics endpoint to");

// Path to the geo ip database, relative to the config store root.
const GEO_IP_PATH: &str = "GeoLite2-Country.mmdb";

// Rate limit for metrics collection (at most once per this interval)
const COLLECT_INTERVAL: Duration = Duration::from_secs(10);

pub(crate) struct MetricsServer {
    handle: Mutex<Option<ScopedAbortHandle>>,
}

impl MetricsServer {
    pub fn new() -> Self {
        Self {
            handle: Mutex::new(None),
        }
    }

    pub async fn init(&self, state: &State) -> Result<()> {
        let entry = state.config.entry(BIND_METRICS_KEY);

        let addr = match entry.get().await {
            Ok(addr) => Some(addr),
            Err(ConfigError::NotFound) => None,
            Err(error) => return Err(error.into()),
        };

        if let Some(addr) = addr {
            let handle = start(state, addr).await?;
            *self.handle.lock().unwrap() = Some(handle);
        }

        Ok(())
    }

    pub async fn bind(&self, state: &State, addr: Option<SocketAddr>) -> Result<()> {
        let entry = state.config.entry(BIND_METRICS_KEY);

        if let Some(addr) = addr {
            let handle = start(state, addr).await?;
            *self.handle.lock().unwrap() = Some(handle);
            entry.set(&addr).await?;
        } else {
            self.handle.lock().unwrap().take();
            entry.remove().await?;
        }

        Ok(())
    }

    pub fn close(&self) {
        self.handle.lock().unwrap().take();
    }
}

async fn start(state: &State, addr: SocketAddr) -> Result<ScopedAbortHandle> {
    let recorder = PrometheusBuilder::new().build_recorder();
    let recorder_handle = recorder.handle();

    let (collect_requester, collect_acceptor) = sync::new(COLLECT_INTERVAL);

    let make_service = make_service_fn(move |_| {
        let recorder_handle = recorder_handle.clone();
        let collect_requester = collect_requester.clone();

        async move {
            Ok::<_, Infallible>(service_fn(move |_| {
                let recorder_handle = recorder_handle.clone();
                let collect_requester = collect_requester.clone();

                async move {
                    collect_requester.request().await;
                    tracing::trace!("Serving metrics");

                    let content = recorder_handle.render();
                    let content = Body::from(content);

                    Ok::<_, Infallible>(Response::new(content))
                }
            }))
        }
    });

    let incoming =
        AddrIncoming::bind(&addr).map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
    tracing::info!("Metrics server listening on {}", incoming.local_addr());

    let acceptor = TlsAcceptor::new(state.get_server_config().await?, incoming);
    let server = Server::builder(acceptor);

    task::spawn(collect(
        collect_acceptor,
        recorder,
        state.network.peer_info_collector(),
        state.config.dir().join(GEO_IP_PATH),
    ));

    let handle = task::spawn(async move {
        if let Err(error) = server.serve(make_service).await {
            tracing::error!(?error, "Metrics server failed");
        }
    })
    .abort_handle()
    .into();

    Ok(handle)
}

async fn collect(
    mut acceptor: sync::Acceptor,
    recorder: PrometheusRecorder,
    peer_info_collector: PeerInfoCollector,
    geo_ip_path: PathBuf,
) {
    let peer_count_key_name = KeyName::from("ouisync_peers_count");
    recorder.describe_gauge(
        peer_count_key_name.clone(),
        Some(Unit::Count),
        "number of active peers".into(),
    );
    let mut peer_count_gauges = GaugeMap::default();

    let collect_duration_key_name = KeyName::from("ouisync_metrics_collect_duration_seconds");
    recorder.describe_gauge(
        collect_duration_key_name.clone(),
        Some(Unit::Seconds),
        "duration of metrics collection".into(),
    );
    let collect_duration_gauge = recorder.register_gauge(
        &Key::from_name(collect_duration_key_name),
        &Metadata::new(module_path!(), Level::INFO, None),
    );

    let mut active_peers = HashMap::<PublicRuntimeId, CountryCode>::default();
    let mut geo_ip = GeoIp::new(geo_ip_path);

    while let Some(_tx) = acceptor.accept().await {
        let start = Instant::now();

        if let Err(error) = geo_ip.refresh().await {
            tracing::error!(
                ?error,
                "Failed to load GeoIP database from {}",
                geo_ip.path().display()
            );
        }

        active_peers.clear();

        for peer in peer_info_collector.collect() {
            let PeerState::Active(id) = peer.state else {
                continue;
            };

            let country = active_peers.entry(id).or_insert(CountryCode::UNKNOWN);
            if *country == CountryCode::UNKNOWN {
                *country = geo_ip
                    .lookup(peer.addr.ip())
                    .unwrap_or(CountryCode::UNKNOWN);
            }
        }

        peer_count_gauges.reset();

        for country in active_peers.values().copied() {
            peer_count_gauges
                .fetch(country, &recorder, &peer_count_key_name)
                .increment(1.0);
        }

        let duration_s = start.elapsed().as_secs_f64();
        collect_duration_gauge.set(duration_s);

        tracing::trace!("Metrics collected in {:.2} s", duration_s);
    }
}

#[derive(Default)]
struct GaugeMap(HashMap<CountryCode, Gauge>);

impl GaugeMap {
    fn fetch(
        &mut self,
        country: CountryCode,
        recorder: &PrometheusRecorder,
        key_name: &KeyName,
    ) -> &Gauge {
        self.0.entry(country).or_insert_with(|| {
            let label = Label::new("country", country.to_string());
            let key = Key::from_parts(key_name.clone(), vec![label]);

            recorder.register_gauge(&key, &Metadata::new(module_path!(), Level::INFO, None))
        })
    }

    fn reset(&self) {
        for gauge in self.0.values() {
            gauge.set(0.0);
        }
    }
}

/// Utilities to request and rate-limit metrics collection.
mod sync {
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
    use tokio::sync::{mpsc, oneshot};

    pub(super) fn new(interval: Duration) -> (Requester, Acceptor) {
        let (tx, rx) = mpsc::channel(1);

        let requester = Requester {
            interval,
            last: Arc::new(Mutex::new(Instant::now())),
            tx,
        };

        let acceptor = Acceptor { rx };

        (requester, acceptor)
    }

    #[derive(Clone)]
    pub(super) struct Requester {
        interval: Duration,
        last: Arc<Mutex<Instant>>,
        tx: mpsc::Sender<oneshot::Sender<()>>,
    }

    impl Requester {
        /// Requests a metrics collection.
        pub async fn request(&self) {
            {
                let mut last = self.last.lock().unwrap();

                if last.elapsed() < self.interval {
                    return;
                } else {
                    *last = Instant::now();
                }
            }

            let (tx, rx) = oneshot::channel();
            self.tx.send(tx).await.ok();
            rx.await.ok();
        }
    }

    pub(super) struct Acceptor {
        rx: mpsc::Receiver<oneshot::Sender<()>>,
    }

    impl Acceptor {
        /// Requests a metrics collection request.
        pub async fn accept(&mut self) -> Option<oneshot::Sender<()>> {
            self.rx.recv().await
        }
    }
}
