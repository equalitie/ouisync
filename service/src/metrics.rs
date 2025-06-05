mod geo_ip;

use crate::{
    config_keys::BIND_METRICS_KEY,
    config_store::{ConfigError, ConfigStore},
    error::Error,
    tls::TlsConfig,
};
use geo_ip::{CountryCode, GeoIp};
use hyper::{server::conn::http1, service::service_fn, Response};
use metrics::{Gauge, Key, KeyName, Label, Level, Metadata, Recorder, Unit};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusRecorder};
use ouisync::{Network, PeerInfoCollector, PeerState, PublicRuntimeId};
use scoped_task::ScopedAbortHandle;
use std::{
    collections::HashMap,
    convert::Infallible,
    io,
    net::SocketAddr,
    path::PathBuf,
    pin::Pin,
    sync::Mutex,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpListener,
    task::{self, JoinSet},
};
use tokio_rustls::TlsAcceptor;

// Path to the geo ip database, relative to the config store root.
const GEO_IP_PATH: &str = "GeoLite2-Country.mmdb";

// Rate limit for metrics collection (at most once per this interval)
const COLLECT_INTERVAL: Duration = Duration::from_secs(10);

pub(crate) struct MetricsServer {
    inner: Mutex<Inner>,
}

struct Inner {
    handle: Option<ScopedAbortHandle>,
}

impl MetricsServer {
    pub async fn init(
        config: &ConfigStore,
        network: &Network,
        tls_config: &TlsConfig,
    ) -> Result<Self, Error> {
        let entry = config.entry(BIND_METRICS_KEY);

        let addr = match entry.get().await {
            Ok(addr) => Some(addr),
            Err(ConfigError::NotFound) => None,
            Err(error) => return Err(error.into()),
        };

        let handle = if let Some(addr) = addr {
            Some(start(config, network, tls_config, addr).await?)
        } else {
            None
        };

        let inner = Inner { handle };

        Ok(Self {
            inner: Mutex::new(inner),
        })
    }

    pub async fn bind(
        &self,
        config: &ConfigStore,
        network: &Network,
        tls_config: &TlsConfig,
        addr: SocketAddr,
    ) -> Result<(), Error> {
        let handle = start(config, network, tls_config, addr).await?;
        self.inner.lock().unwrap().handle = Some(handle);
        config.entry(BIND_METRICS_KEY).set(&addr).await?;

        Ok(())
    }

    pub async fn unbind(&self, config: &ConfigStore) -> Result<(), Error> {
        self.inner.lock().unwrap().handle = None;
        config.entry(BIND_METRICS_KEY).remove().await?;

        Ok(())
    }

    pub fn close(&self) {
        self.inner.lock().unwrap().handle = None;
    }
}

async fn start(
    config: &ConfigStore,
    network: &Network,
    tls_config: &TlsConfig,
    addr: SocketAddr,
) -> Result<ScopedAbortHandle, Error> {
    let recorder = PrometheusBuilder::new().build_recorder();
    let recorder_handle = recorder.handle();

    let (collect_requester, collect_acceptor) = sync::new(COLLECT_INTERVAL);

    let tcp_listener = TcpListener::bind(&addr).await.map_err(Error::Bind)?;

    match tcp_listener.local_addr() {
        Ok(addr) => tracing::info!("Metrics server listening on {addr}"),
        Err(error) => tracing::error!(
            ?error,
            "Metrics server failed to retrieve the listening address"
        ),
    }

    let tls_acceptor = TlsAcceptor::from(tls_config.server().await?);

    task::spawn(collect(
        collect_acceptor,
        recorder,
        network.peer_info_collector(),
        config.dir().join(GEO_IP_PATH),
    ));

    let handle = task::spawn(async move {
        let mut tasks = JoinSet::new();

        loop {
            let (stream, addr) = match tcp_listener.accept().await {
                Ok(conn) => conn,
                Err(error) => {
                    tracing::error!(?error, "Metrics server failed to accept new connection");
                    break;
                }
            };

            let stream = match tls_acceptor.accept(stream).await {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        %addr,
                        "Metrics server failed to perform TLS handshake"
                    );
                    continue;
                }
            };

            let recorder_handle = recorder_handle.clone();
            let collect_requester = collect_requester.clone();

            tasks.spawn(async move {
                let service = move |_req| {
                    let recorder_handle = recorder_handle.clone();
                    let collect_requester = collect_requester.clone();

                    async move {
                        collect_requester.request().await;
                        tracing::trace!("Serving metrics");

                        let content = recorder_handle.render();

                        Ok::<_, Infallible>(Response::new(content))
                    }
                };

                match http1::Builder::new()
                    .serve_connection(StreamCompat(stream), service_fn(service))
                    .await
                {
                    Ok(()) => (),
                    Err(error) => {
                        tracing::error!(?error, %addr, "Metrics server connection failed")
                    }
                }
            });
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
            let PeerState::Active { id, .. } = peer.state else {
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

// hyper no longer accepts `tokio::AsyncRead` and `tokio::AsyncWrite` and uses its own traits
// instead so we now need this compatibility wrapper.
struct StreamCompat<T>(T);

impl<T> hyper::rt::Read for StreamCompat<T>
where
    T: AsyncRead + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<Result<(), io::Error>> {
        unsafe {
            let mut buf = ReadBuf::uninit(buf.as_mut());

            let n = match Pin::new(&mut self.0).poll_read(cx, &mut buf) {
                Poll::Ready(Ok(())) => buf.filled().len(),
                other => return other,
            };

            buf.advance(n);
        }

        Poll::Ready(Ok(()))
    }
}

impl<T> hyper::rt::Write for StreamCompat<T>
where
    T: AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.0).poll_write_vectored(cx, bufs)
    }
}
