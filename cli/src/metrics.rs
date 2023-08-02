use crate::state::State;
use hyper::{
    server::Server,
    service::{make_service_fn, service_fn},
    Body, Response,
};
use metrics::{Key, KeyName, Recorder, Unit};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusRecorder};
use ouisync_bridge::{
    config::{ConfigError, ConfigKey},
    error::Error,
};
use ouisync_lib::{network::PeerState, PeerInfoCollector, PublicRuntimeId};
use scoped_task::ScopedAbortHandle;
use std::{
    collections::HashSet, convert::Infallible, io, net::SocketAddr, sync::Mutex, time::Duration,
};
use tokio::{
    task,
    time::{self, MissedTickBehavior},
};

const BIND_METRICS_KEY: ConfigKey<SocketAddr> =
    ConfigKey::new("bind_metrics", "Addresses to bind the metrics endpoint to");

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

    let collect = collect(recorder, state.network.peer_info_collector());

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

async fn collect(recorder: PrometheusRecorder, peer_info_collector: PeerInfoCollector) {
    let peers_count = {
        let key_name = KeyName::from("peers_count");
        let key = Key::from_name(key_name.clone());

        recorder.describe_gauge(key_name, Some(Unit::Count), "number of active peers".into());
        recorder.register_gauge(&key)
    };

    // TODO: do this on request instead of peridically, but aggregate simultaneous requests
    let mut interval = time::interval(COLLECT_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let mut peers: HashSet<PublicRuntimeId> = HashSet::new();

    loop {
        interval.tick().await;

        peers.clear();

        for peer in peer_info_collector.collect() {
            let PeerState::Active(id) = peer.state else {
                continue;
            };

            // TODO: geoip lookup

            peers.insert(id);
        }

        peers_count.set(peers.len() as f64);
    }
}
