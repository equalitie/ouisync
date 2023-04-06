use super::ip;
use crate::{
    collections::{hash_map, HashMap},
    deadlock::BlockingMutex,
    state_monitor::StateMonitor,
};
use chrono::{offset::Local, DateTime};
use futures_util::TryStreamExt;
use futures_util::{Stream, StreamExt};
use rupnp::{
    http::Uri,
    ssdp::{SearchTarget, URN},
    Device, Service,
};
use scoped_task::ScopedJoinHandle;
use std::{
    fmt,
    future::Future,
    io, net,
    sync::{Arc, Weak},
    time::SystemTime,
};
use tokio::{
    sync::watch,
    time::{sleep, Duration, Instant},
};
use tracing::{Instrument, Span};

type JobHandles = HashMap<Uri, TrackedDevice>;

struct TrackedDevice {
    last_seen: Instant,
    // `Option` is used to keep track of which Uris have already been tried so as to not flood the
    // debug log with repeated "device failure" warnings.
    job: Option<ScopedJoinHandle<()>>,
}

pub(crate) struct PortForwarder {
    mappings: Arc<BlockingMutex<Mappings>>,
    on_change_tx: Arc<watch::Sender<()>>,
    task: BlockingMutex<Weak<ScopedJoinHandle<()>>>,
    monitor: StateMonitor,
    span: Span,
}

impl PortForwarder {
    pub fn new(monitor: StateMonitor) -> Self {
        let mappings = Arc::new(BlockingMutex::new(Default::default()));
        let (on_change_tx, _) = watch::channel(());

        Self {
            mappings,
            on_change_tx: Arc::new(on_change_tx),
            task: BlockingMutex::new(Weak::new()),
            monitor,
            span: Span::current(),
        }
    }

    pub fn add_mapping(&self, internal: u16, external: u16, protocol: ip::Protocol) -> Mapping {
        let data = MappingData {
            internal,
            external,
            protocol,
        };

        let is_new_mapping = match self.mappings.lock().unwrap().entry(data) {
            hash_map::Entry::Occupied(mut entry) => {
                *entry.get_mut() += 1;
                false
            }
            hash_map::Entry::Vacant(entry) => {
                tracing::info!(
                    parent: &self.span,
                    "UPnP starting port forwarding EXT:{} -> INT:{} ({})",
                    external,
                    internal,
                    protocol,
                );
                entry.insert(1);
                true
            }
        };

        // Start the forwarder when the first mapping is created and stop it when the last mapping
        // is destroyed.
        let mut task_lock = self.task.lock().unwrap();

        let task = if let Some(task) = task_lock.upgrade() {
            task
        } else {
            let mappings = self.mappings.clone();
            let on_change_rx = self.on_change_tx.subscribe();
            let monitor = self.monitor.clone();

            let task = async move {
                let result = Self::run(mappings, on_change_rx, monitor).await;
                // Warning, because we don't actually expect this to happen.
                tracing::warn!("UPnP port forwarding ended ({:?})", result)
            };
            let task = task.instrument(self.span.clone());
            let task = scoped_task::spawn(task);
            let task = Arc::new(task);
            *task_lock = Arc::downgrade(&task);

            task
        };

        if is_new_mapping {
            // We need to do this _after_ the subscription above, otherwise `on_change_rx` won't
            // receive it.
            self.on_change_tx.send(()).unwrap_or(());
        }

        Mapping {
            data,
            on_change_tx: self.on_change_tx.clone(),
            mappings: self.mappings.clone(),
            _task: task,
            span: self.span.clone(),
        }
    }

    async fn run(
        mappings: Arc<BlockingMutex<Mappings>>,
        on_change_rx: watch::Receiver<()>,
        monitor: StateMonitor,
    ) -> Result<(), rupnp::Error> {
        // Devices may have a timeout period when they don't respond to repeated queries, the
        // DISCOVERY_RUDATION constant should be higher than that. The rupnp project internally
        // assumes this duration is three seconds.
        const DISCOVERY_DURATION: Duration = Duration::from_secs(5);
        // SLEEP_DURATION is relevant e.g. when the user enables UPnP on their router. We don't
        // want them to have to wait too long to start syncing after they do so. But we also have
        // to balance it with not spamming the network with multicast queries.
        const SLEEP_DURATION: Duration = Duration::from_secs(15);

        const ERROR_SLEEP_DURATION: Duration = Duration::from_secs(5);

        let job_handles = Arc::new(BlockingMutex::new(JobHandles::default()));

        let devices_monitor = monitor.make_child("devices");
        let lookup_counter = monitor.make_value("lookup counter", 0);
        let mut error_logged = false;

        // Periodically check for new devices: maybe UPnP was enabled later after the app started,
        // maybe the device was swapped for another one,...
        loop {
            *lookup_counter.get() += 1;

            // Cleanup: remove devices not seen in a while.
            job_handles.lock().unwrap().retain(|_uri, device| {
                // Unwrap OK because we're not anywhere near the epoch.
                device.last_seen
                    >= Instant::now()
                        .checked_sub((DISCOVERY_DURATION + SLEEP_DURATION) * 3)
                        .unwrap()
            });

            let mut device_urls =
                match discover_device_urls(&SearchTarget::RootDevice, DISCOVERY_DURATION).await {
                    Ok(device_urls) => {
                        error_logged = false;
                        Box::pin(device_urls)
                    }
                    Err(e) => {
                        // This happens e.g. when a device has no connection to the internet. Let's log
                        // the information but keep looping for when we get back online.
                        if !error_logged {
                            tracing::debug!("Error discovering device URLs: {:?}", e);
                            error_logged = true;
                        }
                        sleep(ERROR_SLEEP_DURATION).await;
                        continue;
                    }
                };

            // NOTE: don't use `try_next().await?` here from the TryStreamExt interface as that
            // will quit on the first device that fails to respond. Whereas what we want is to just
            // ignore the device and go on trying the next one in line.
            while let Some(result) = device_urls.next().await {
                let device_url = match result {
                    Ok(device_url) => device_url,
                    Err(e) => {
                        tracing::debug!("Failed to contact one of the UPnP devices: {:?}", e);
                        continue;
                    }
                };

                let on_change_rx = on_change_rx.clone();
                let mappings = mappings.clone();
                let devices_monitor = devices_monitor.clone();

                Self::spawn_if_not_running(device_url.clone(), &job_handles, move || {
                    async move {
                        let device = Device::from_url(device_url).await?;

                        // We are only interested in IGD.
                        if device.device_type().typ() != "InternetGatewayDevice" {
                            return Ok(());
                        }

                        if let Some(service) = find_connection_service(&device) {
                            let per_igd_port_forwarder = PerIGDPortForwarder {
                                device_url: device.url().clone(),
                                service,
                                on_change_rx,
                                mappings,
                                active_mappings: Default::default(),
                                monitor: devices_monitor.make_child(device.friendly_name()),
                            };

                            per_igd_port_forwarder.run().await;

                            // The above is expected to never exit. But even if it does (e.g. if
                            // the code changes), it should be Ok because the device is re-spawned
                            // the next time the discovery finds it.
                            Ok(())
                        } else {
                            Err(rupnp::Error::InvalidResponse(
                                format!(
                                    "port forwarding failed: non-compliant device '{}'",
                                    device.friendly_name()
                                )
                                .into(),
                            ))
                        }
                    }
                    .instrument(Span::current())
                });
            }

            tokio::time::sleep(SLEEP_DURATION).await;
        }
    }

    fn spawn_if_not_running<JobMaker, JobFuture>(
        device_url: Uri,
        job_handles: &Arc<BlockingMutex<JobHandles>>,
        job: JobMaker,
    ) where
        JobMaker: FnOnce() -> JobFuture + Send + 'static,
        JobFuture: Future<Output = Result<(), rupnp::Error>> + Send,
    {
        let weak_handles = Arc::downgrade(job_handles);

        let mut job_handles = job_handles.lock().unwrap();

        let do_spawn = |first_attempt| {
            let device_url = device_url.clone();

            scoped_task::spawn(async move {
                let result = job().await;

                if first_attempt {
                    if let Err(e) = result {
                        tracing::warn!("UPnP port forwarding on IGD {:?} ended: {}", device_url, e);
                    }
                }

                if let Some(handles) = weak_handles.upgrade() {
                    if let Some(dev) = handles.lock().unwrap().get_mut(&device_url) {
                        dev.job = None;
                    }
                }
            })
        };

        use crate::collections::hash_map::Entry;

        match job_handles.entry(device_url.clone()) {
            Entry::Occupied(mut entry) => {
                let dev = entry.get_mut();
                dev.last_seen = Instant::now();
                if dev.job.is_none() {
                    dev.job = Some(do_spawn(false));
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(TrackedDevice {
                    last_seen: Instant::now(),
                    job: Some(do_spawn(true)),
                });
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct MappingData {
    pub internal: u16,
    pub external: u16,
    pub protocol: ip::Protocol,
}

// The map value is a reference counter.
type Mappings = HashMap<MappingData, usize>;

pub(crate) struct Mapping {
    data: MappingData,
    on_change_tx: Arc<watch::Sender<()>>,
    mappings: Arc<BlockingMutex<Mappings>>,
    _task: Arc<ScopedJoinHandle<()>>,
    span: Span,
}

impl Drop for Mapping {
    fn drop(&mut self) {
        tracing::info!(
            parent: &self.span,
            "UPnP stopping port forwarding EXT:{} -> INT:{} ({})",
            self.data.external,
            self.data.internal,
            self.data.protocol,
        );

        let mut mappings = self.mappings.lock().unwrap();

        match mappings.entry(self.data) {
            hash_map::Entry::Occupied(mut entry) => {
                let refcount = entry.get_mut();
                *refcount -= 1;

                if *refcount == 0 {
                    entry.remove();
                    self.on_change_tx.send(()).unwrap_or(());
                }
            }
            hash_map::Entry::Vacant(_) => {
                unreachable!();
            }
        }
    }
}

struct PerIGDPortForwarder {
    device_url: Uri,
    service: Service,
    on_change_rx: watch::Receiver<()>,
    mappings: Arc<BlockingMutex<Mappings>>,
    active_mappings: BlockingMutex<HashMap<MappingData, ScopedJoinHandle<()>>>,
    monitor: StateMonitor,
}

impl PerIGDPortForwarder {
    async fn run(mut self) {
        let _ext_ip_task = self.start_ext_ip_discovery();

        let _url_monitor = self.monitor.make_value("url", self.device_url.clone());
        let local_ip_monitor = self.monitor.make_value("local ip", None);

        let local_ip = loop {
            let local_ip = local_address_to(&self.device_url).await;
            *local_ip_monitor.get() = local_ip.as_ref().ok().copied();

            match local_ip {
                Ok(local_ip) => break local_ip,
                Err(_) => {
                    sleep(Duration::from_secs(30)).await;
                    continue;
                }
            }
        };

        let mappings_monitor = self.monitor.make_child("Mappings");

        while self.on_change_rx.changed().await.is_ok() {
            let new_mappings = self.mappings.lock().unwrap();
            let mut active_mappings = self.active_mappings.lock().unwrap();

            // Remove from `active_mappings` those that are not in in `new_mappings`.
            active_mappings.retain(|k, _| new_mappings.contains_key(k));

            // Add to `active_mappings` those that are `active_mappings`.
            for k in new_mappings.keys() {
                if let hash_map::Entry::Vacant(entry) = active_mappings.entry(*k) {
                    entry.insert(self.activate_mapping(*k, local_ip, &mappings_monitor));
                }
            }
        }
    }

    fn activate_mapping(
        &self,
        data: MappingData,
        local_ip: net::IpAddr,
        mappings_monitor: &StateMonitor,
    ) -> ScopedJoinHandle<()> {
        let service = self.service.clone();
        let device_uri = self.device_url.clone();
        let mapping_monitor = mappings_monitor.make_child(format!(
            "{} EXT:{} -> INT:{}",
            data.protocol, data.external, data.internal,
        ));

        scoped_task::spawn(async move {
            Self::run_mapping(data, local_ip, service, device_uri, mapping_monitor)
                .instrument(Span::current())
                .await;
            unreachable!();
        })
    }

    fn start_ext_ip_discovery(&self) -> ScopedJoinHandle<()> {
        let service = self.service.clone();
        let device_url = self.device_url.clone();
        let external_ip = self.monitor.make_value("external ip", None);

        scoped_task::spawn(async move {
            loop {
                *external_ip.get() = get_external_ip_address(&service, &device_url).await.ok();
                sleep(Duration::from_secs(4 * 60)).await;
            }
        })
    }

    async fn run_mapping(
        mapping: MappingData,
        local_ip: net::IpAddr,
        service: Service,
        device_url: Uri,
        monitor: StateMonitor,
    ) {
        let lease_duration = Duration::from_secs(5 * 60);
        let sleep_delta = Duration::from_secs(5);
        let sleep_duration = lease_duration.saturating_sub(sleep_delta);
        let error_sleep_duration = Duration::from_secs(10);

        let mut ext_port_reported = false;

        // Monitoring
        let iteration = monitor.make_value("iteration", 0);
        let state = monitor.make_value("state", State::Start);

        loop {
            *iteration.get() += 1;
            *state.get() = State::AddingPortMappingFirstStage;

            if let Err(err) =
                add_port_mappings(&service, &device_url, &local_ip, lease_duration, &mapping).await
            {
                *state.get() = State::StageOneFailure(err);
                sleep(error_sleep_duration).await;
                continue;
            }

            if !ext_port_reported {
                ext_port_reported = true;

                tracing::info!(
                    "UPnP port forwarding started on external port {}:{}",
                    mapping.protocol,
                    mapping.external
                );
            }

            *state.get() = State::SleepingFirstStage((SystemTime::now() + sleep_duration).into());
            sleep(sleep_duration).await;

            *state.get() = State::AddingPortMappingSecondStage;
            // We've seen IGD devices that refuse to update the lease if the previous lease has not
            // yet ended. So what we're doing here is that we do attempt to do just that, but we
            // also then wait until we know the previous lease ended and bump the lease again.
            //
            // This way, in the worst case, the buggy devices shall be unreachable with probability
            //
            // 2*sleep_delta/(lease_duration-sleep_delta)
            //
            // Hopefully that can be remedied by the client retrying the connection establishment.
            //
            // Note that there are two other possible workarounds for this problem, though neither
            // of them is perfect:
            //
            // 1. If the lease_duration is zero, IGD devices _should_ interpret that as indefinite.
            //    however we've seen devices which refuse to accept zero as the lease duration.
            // 2. We could try to update the lease, and then confirm that the lease has indeed been
            //    updated. Unfortunately, we've seen IGD devices which fail to report active
            //    leases (or report only the first one in their list or some other random subset).
            if let Err(err) =
                add_port_mappings(&service, &device_url, &local_ip, lease_duration, &mapping).await
            {
                *state.get() = State::StageTwoFailure(err);
                sleep(error_sleep_duration).await;
                continue;
            }

            *state.get() = State::SleepingSecondStage((SystemTime::now() + 2 * sleep_delta).into());
            sleep(sleep_delta * 2).await;
        }
    }
}

// For IGDv1 see Section 2.4.16 in
// https://openconnectivity.org/wp-content/uploads/2015/11/UPnP_IGD_WANIPConnection-1.0.pdf
//
// For IGDv2 see Section 2.5.16 in
// https://upnp.org/specs/gw/UPnP-gw-WANIPConnection-v2-Service.pdf
//
// TODO: Consider also implementing AddAnyPortMapping when on IGDv2 for cases when the
// requested external port is not free.
async fn add_port_mappings(
    service: &Service,
    device_url: &Uri,
    local_ip: &net::IpAddr,
    lease_duration: Duration,
    mapping: &MappingData,
) -> Result<(), rupnp::Error> {
    let lease_duration = if lease_duration == Duration::ZERO {
        Duration::ZERO
    } else {
        std::cmp::max(Duration::from_secs(1), lease_duration)
    };

    const MAPPING_DESCRIPTION: &str = "OuiSync";

    let args = format!(
        "<NewRemoteHost></NewRemoteHost>\
         <NewEnabled>1</NewEnabled>\
         <NewProtocol>{}</NewProtocol>\
         <NewExternalPort>{}</NewExternalPort>\
         <NewInternalPort>{}</NewInternalPort>\
         <NewInternalClient>{}</NewInternalClient>\
         <NewPortMappingDescription>{}</NewPortMappingDescription>\
         <NewLeaseDuration>{}</NewLeaseDuration>",
        mapping.protocol,
        mapping.external,
        mapping.internal,
        local_ip,
        MAPPING_DESCRIPTION,
        lease_duration.as_secs()
    );

    service.action(device_url, "AddPortMapping", &args).await?;

    Ok(())
}

// For IGDv1 see Section 2.4.18 in
// https://openconnectivity.org/wp-content/uploads/2015/11/UPnP_IGD_WANIPConnection-1.0.pdf
//
// For IGDv2 see Section 2.5.20 in
// https://upnp.org/specs/gw/UPnP-gw-WANIPConnection-v2-Service.pdf
async fn get_external_ip_address(
    service: &Service,
    device_url: &Uri,
) -> Result<net::IpAddr, rupnp::Error> {
    service
        .action(device_url, "GetExternalIPAddress", "")
        .await?
        .get("NewExternalIPAddress")
        .ok_or(InvalidResponse(
            "response has no NewExternalIPAddress field",
        ))?
        .parse::<net::IpAddr>()
        .map_err(|_| InvalidResponse("failed to parse IP address").into())
}

enum State {
    Start,
    AddingPortMappingFirstStage,
    StageOneFailure(rupnp::Error),
    // Contains the wakeup time.
    SleepingFirstStage(DateTime<Local>),
    AddingPortMappingSecondStage,
    // Contains the wakeup time.
    SleepingSecondStage(DateTime<Local>),
    StageTwoFailure(rupnp::Error),
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Start => {
                write!(f, "Start")
            }
            Self::AddingPortMappingFirstStage => {
                write!(f, "AddingPortMappingFirstStage")
            }
            Self::StageOneFailure(err) => {
                write!(f, "StageOneFailure({:?})", err)
            }
            Self::SleepingFirstStage(wake_up_time) => {
                write!(f, "SleepingFirstStage {}", wake_up_time.format("%T"))
            }
            Self::AddingPortMappingSecondStage => {
                write!(f, "AddingPortMappingSecondStage")
            }
            Self::SleepingSecondStage(wake_up_time) => {
                write!(f, "SleepingSecondStage {}", wake_up_time.format("%T"))
            }
            Self::StageTwoFailure(err) => {
                write!(f, "StageTwoFailure({:?})", err)
            }
        }
    }
}

const UPNP_SCHEMA: &str = "schemas-upnp-org";

fn find_device_any_version<'a>(
    device: &'a rupnp::DeviceSpec,
    device_type: &'static str,
) -> Option<&'a rupnp::DeviceSpec> {
    device
        .find_device(&URN::device(UPNP_SCHEMA, device_type, 2))
        .or_else(|| device.find_device(&URN::device(UPNP_SCHEMA, device_type, 1)))
}

fn find_service_any_version<'a>(
    device: &'a rupnp::DeviceSpec,
    service_type: &'static str,
) -> Option<&'a rupnp::Service> {
    device
        .find_service(&URN::service(UPNP_SCHEMA, service_type, 2))
        .or_else(|| device.find_service(&URN::service(UPNP_SCHEMA, service_type, 1)))
}

fn find_connection_service(device: &rupnp::DeviceSpec) -> Option<rupnp::Service> {
    find_device_any_version(device, "WANDevice")
        .and_then(|device| find_device_any_version(device, "WANConnectionDevice"))
        .and_then(|device| {
            find_service_any_version(device, "WANIPConnection")
                .or_else(|| find_service_any_version(device, "WANPPPConnection"))
        })
        .cloned()
}

async fn local_address_to(url: &Uri) -> io::Result<net::IpAddr> {
    use std::io::{Error, ErrorKind};
    use std::net::SocketAddr;

    let remote_addr = {
        let host = url.host().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("Failed to get the host part from URL {:?}", url),
            )
        })?;

        let addr = host.parse::<net::IpAddr>().map_err(|_| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("Failed to parse IP from URL {:?}", url),
            )
        })?;

        let port = url.port_u16().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("Failed to parse PORT from URL {:?}", url),
            )
        })?;

        SocketAddr::new(addr, port)
    };

    let any: SocketAddr = {
        if remote_addr.ip().is_ipv4() {
            (net::Ipv4Addr::UNSPECIFIED, 0).into()
        } else {
            (net::Ipv6Addr::UNSPECIFIED, 0).into()
        }
    };

    let socket = tokio::net::UdpSocket::bind(any).await?;
    socket.connect(remote_addr).await?;
    socket.local_addr().map(|addr| addr.ip())
}

#[derive(Debug)]
struct InvalidResponse(&'static str);

impl From<InvalidResponse> for rupnp::Error {
    fn from(err: InvalidResponse) -> Self {
        rupnp::Error::invalid_response(err)
    }
}

impl fmt::Display for InvalidResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for InvalidResponse {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

// There is a problem with the rupnp::discover function in that once the ssdp_client::search finds
// device URLs, it then starts probing them (with Device::from_url) one by one. If there is a
// device on the network which does not respond, or does so after a very long timeout (not affected
// by the `timeout` argument), then all devices that follow will be probed after the long delay or
// never.
//
// So instead we do the ssdp_client::search ourselves and then the Device::from_url in a separate
// coroutine spawned above.
pub async fn discover_device_urls(
    search_target: &SearchTarget,
    timeout: Duration,
) -> Result<impl Stream<Item = Result<Uri, rupnp::Error>>, rupnp::Error> {
    Ok(ssdp_client::search(search_target, timeout, 3)
        .await?
        .map_err(rupnp::Error::SSDPError)
        .map(|res| Ok(res?.location().parse()?)))
}
