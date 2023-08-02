use crate::{
    geo_ip::{CountryCode, GeoIp},
    state::State,
};
use hyper::{
    server::Server,
    service::{make_service_fn, service_fn},
    Body, Response,
};
use metrics::{Gauge, Key, KeyName, Label, Recorder, Unit};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusRecorder};
use ouisync_bridge::{
    config::{ConfigError, ConfigKey},
    error::Error,
};
use ouisync_lib::{network::PeerState, PeerInfoCollector, PublicRuntimeId};
use scoped_task::ScopedAbortHandle;
use std::{
    collections::HashMap, convert::Infallible, io, net::SocketAddr, path::PathBuf, sync::Mutex,
    time::Duration,
};
use tokio::{
    task,
    time::{self, MissedTickBehavior},
};

const BIND_METRICS_KEY: ConfigKey<SocketAddr> =
    ConfigKey::new("bind_metrics", "Addresses to bind the metrics endpoint to");

// Path to the geo ip database, relative to the config store root.
const GEO_IP_PATH: &str = "GeoLite2-Country.mmdb";

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

    pub async fn init(&self, state: &State) -> Result<(), Error> {
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

    pub async fn bind(&self, state: &State, addr: Option<SocketAddr>) -> Result<(), Error> {
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

async fn start(state: &State, addr: SocketAddr) -> Result<ScopedAbortHandle, Error> {
    let recorder = PrometheusBuilder::new().build_recorder();
    let recorder_handle = recorder.handle();

    let make_service = make_service_fn(move |_| {
        let recorder_handle = recorder_handle.clone();

        async move {
            Ok::<_, Infallible>(service_fn(move |_| {
                let recorder_handle = recorder_handle.clone();
                async move { Ok::<_, Infallible>(Response::new(Body::from(recorder_handle.render()))) }
            }))
        }
    });

    let server =
        Server::try_bind(&addr).map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;

    let geo_ip_path = state.config.dir().join(GEO_IP_PATH);
    let collect = collect(recorder, state.network.peer_info_collector(), geo_ip_path);

    let handle = task::spawn(async move {
        let _collect_handle = scoped_task::spawn(collect);

        if let Err(error) = server.serve(make_service).await {
            tracing::error!(?error, "Metrics server failed");
        }
    })
    .abort_handle()
    .into();

    Ok(handle)
}

async fn collect(
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

    let mut geo_ip = GeoIp::new(geo_ip_path);

    // TODO: do this on request instead of peridically, but aggregate simultaneous requests
    let mut interval = time::interval(COLLECT_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let mut active_peers = HashMap::<PublicRuntimeId, CountryCode>::default();

    loop {
        interval.tick().await;

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
                *country = geo_ip.lookup(peer.ip).unwrap_or(CountryCode::UNKNOWN);
            }
        }

        peer_count_gauges.reset();

        for country in active_peers.values().copied() {
            peer_count_gauges
                .fetch(country, &recorder, &peer_count_key_name)
                .increment(1.0);
        }
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

            recorder.register_gauge(&key)
        })
    }

    fn reset(&self) {
        for gauge in self.0.values() {
            gauge.set(0.0);
        }
    }
}
