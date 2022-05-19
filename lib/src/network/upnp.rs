use super::ip_stack::Protocol;
use crate::{
    scoped_task::{self, ScopedJoinHandle},
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
use std::{
    collections::{hash_map, HashMap},
    fmt,
    future::Future,
    io, net,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use tokio::time::sleep;

// `Option` is used to keep track of which Uris have already been tried so as to not flood the
// debug log with repeated "device failure" warnings.
type JobHandles = HashMap<Uri, Option<ScopedJoinHandle<()>>>;

pub(crate) struct PortForwarder {
    mappings: Arc<Mutex<Mappings>>,
    _task: ScopedJoinHandle<()>,
}

impl PortForwarder {
    pub fn new(monitor: Arc<StateMonitor>) -> Self {
        let mappings = Arc::new(Mutex::new(HashMap::new()));

        let task = scoped_task::spawn({
            let mappings = mappings.clone();

            async move {
                let result = Self::run(mappings, monitor).await;
                // Warning, because we don't actually expect this to happen.
                log::warn!("UPnP port forwarding ended ({:?})", result)
            }
        });

        Self {
            mappings,
            _task: task,
        }
    }

    pub fn add_mapping(&self, internal: u16, external: u16, protocol: Protocol) -> Mapping {
        let data = MappingData {
            internal,
            external,
            protocol,
        };

        let mut mappings = self.mappings.lock().unwrap();

        match mappings.entry(data) {
            hash_map::Entry::Occupied(mut entry) => {
                *entry.get_mut() += 1;
            }
            hash_map::Entry::Vacant(entry) => {
                log::info!(
                    "UPnP starting port forwarding EXT:{} -> INT:{} ({})",
                    external,
                    internal,
                    protocol,
                );
                entry.insert(1);
            }
        }

        Mapping {
            data,
            mappings: self.mappings.clone(),
        }
    }

    async fn run(
        mappings: Arc<Mutex<Mappings>>,
        monitor: Arc<StateMonitor>,
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

        let job_handles = Arc::new(Mutex::new(JobHandles::new()));

        let lookup_counter = monitor.make_value::<u64>("lookup_counter".into(), 0);
        let monitor = monitor.make_child("Devices");

        let mut error_logged = false;

        // Periodically check for new devices: maybe UPnP was enabled later after the app started,
        // maybe the device was swapped for another one,...
        loop {
            *lookup_counter.get() += 1;

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
                            log::debug!("Error discovering device URLs: {:?}", e);
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
                        log::debug!("Failed to contact one of the UPnP devices: {:?}", e);
                        continue;
                    }
                };

                let mappings = mappings.clone();
                let monitor = monitor.clone();

                Self::spawn_if_not_running(device_url.clone(), &job_handles, move || {
                    Box::pin(async move {
                        let device = Device::from_url(device_url).await?;

                        // We are only interested in IGD.
                        if device.device_type().typ() != "InternetGatewayDevice" {
                            return Ok(());
                        }

                        if let Some(service) = find_connection_service(&device) {
                            let monitor = monitor.make_child(device.friendly_name());
                            let url = device.url().clone();

                            let per_igd_port_forwarder = PerIGDPortForwarder {
                                device_url: url.clone(),
                                service,
                                mappings,
                                monitor,
                            };

                            per_igd_port_forwarder.run().await
                        } else {
                            Err(rupnp::Error::InvalidResponse(
                                format!(
                                    "port forwarding failed: non-compliant device '{}'",
                                    device.friendly_name()
                                )
                                .into(),
                            ))
                        }
                    })
                });
            }

            tokio::time::sleep(SLEEP_DURATION).await;
        }
    }
    fn spawn_if_not_running<JobMaker>(
        device_url: Uri,
        job_handles: &Arc<Mutex<JobHandles>>,
        job: JobMaker,
    ) where
        JobMaker: FnOnce() -> Pin<Box<dyn Future<Output = Result<(), rupnp::Error>> + Send>>
            + Send
            + 'static,
    {
        let weak_handles = Arc::downgrade(job_handles);

        let mut job_handles = job_handles.lock().unwrap();

        let do_spawn = |first_attempt| {
            let device_url = device_url.clone();

            scoped_task::spawn(async move {
                let result = job().await;

                if first_attempt {
                    if let Err(e) = result {
                        log::warn!("UPnP port forwarding on IGD {:?} ended: {}", device_url, e);
                    }
                }

                if let Some(handles) = weak_handles.upgrade() {
                    handles.lock().unwrap().insert(device_url, None);
                }
            })
        };

        use std::collections::hash_map::Entry;

        match job_handles.entry(device_url.clone()) {
            Entry::Occupied(mut entry) => {
                if entry.get().is_none() {
                    entry.insert(Some(do_spawn(false)));
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(Some(do_spawn(true)));
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct MappingData {
    pub internal: u16,
    pub external: u16,
    pub protocol: Protocol,
}

// The map value is a reference counter.
type Mappings = HashMap<MappingData, usize>;

pub(crate) struct Mapping {
    data: MappingData,
    mappings: Arc<Mutex<Mappings>>,
}

impl Drop for Mapping {
    fn drop(&mut self) {
        let mut mappings = self.mappings.lock().unwrap();

        match mappings.entry(self.data) {
            hash_map::Entry::Occupied(mut entry) => {
                let refcount = entry.get_mut();
                *refcount -= 1;

                if *refcount == 0 {
                    entry.remove();
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
    mappings: Arc<Mutex<Mappings>>,
    monitor: Arc<StateMonitor>,
}

impl PerIGDPortForwarder {
    async fn run(&self) -> Result<(), rupnp::Error> {
        // NOTE: It's OK to exit this function when an error happens (as opposed to keep looping).
        // It's because we're periodically searching for IGD devices and as long as there is one,
        // this task (if it fails) shall be restarted.
        let local_ip = local_address_to(&self.device_url).await?;

        let lease_duration = Duration::from_secs(5 * 60);
        let sleep_delta = Duration::from_secs(5);
        let sleep_duration = lease_duration.saturating_sub(sleep_delta);

        let mut error_reported = false;
        let mut ext_port_reported = false;
        let mut ext_addr_reported = false;

        // Monitoring
        let state = self.monitor.make_value("state".into(), State::Start);
        let ext_ip = self.monitor.make_value("external_ip".into(), None);
        let iteration = self.monitor.make_value("iteration".into(), 0);
        let _local_ip = self.monitor.make_value("local_ip".into(), local_ip);

        loop {
            let mappings = self.mappings_snapshot();

            *iteration.get() += 1;
            *state.get() = State::AddingPortMappingFirstStage;

            self.add_port_mappings(&local_ip, lease_duration, &mappings)
                .await?;

            if !ext_port_reported {
                ext_port_reported = true;

                for mapping in &mappings {
                    log::info!(
                        "UPnP port forwarding started on external port {}:{} {}",
                        mapping.protocol,
                        mapping.external,
                        mapping.protocol
                    );
                }
            }

            *state.get() = State::GettingIpAddress;
            match self.get_external_ip_address().await {
                Ok(addr) => {
                    *ext_ip.get() = Some(addr);

                    if !ext_addr_reported {
                        error_reported = false;
                        ext_addr_reported = true;
                        log::info!("UPnP the external IP address is {}", addr);
                    }
                }
                Err(e) => {
                    *ext_ip.get() = None;

                    if !error_reported {
                        error_reported = true;
                        ext_addr_reported = false;
                        log::warn!("UPnP failed to retrieve external IP address: {:?}", e);
                    }
                }
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
            self.add_port_mappings(&local_ip, lease_duration, &mappings)
                .await?;

            *state.get() = State::SleepingSecondStage((SystemTime::now() + 2 * sleep_delta).into());
            sleep(sleep_delta * 2).await;
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
        &self,
        local_ip: &net::IpAddr,
        lease_duration: Duration,
        mappings: &Vec<MappingData>,
    ) -> Result<(), rupnp::Error> {
        let lease_duration = if lease_duration == Duration::ZERO {
            Duration::ZERO
        } else {
            std::cmp::max(Duration::from_secs(1), lease_duration)
        };

        const MAPPING_DESCRIPTION: &str = "OuiSync";

        for mapping in mappings {
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

            self.service
                .action(&self.device_url, "AddPortMapping", &args)
                .await?;
        }

        Ok(())
    }

    // For IGDv1 see Section 2.4.18 in
    // https://openconnectivity.org/wp-content/uploads/2015/11/UPnP_IGD_WANIPConnection-1.0.pdf
    //
    // For IGDv2 see Section 2.5.20 in
    // https://upnp.org/specs/gw/UPnP-gw-WANIPConnection-v2-Service.pdf
    async fn get_external_ip_address(&self) -> Result<net::IpAddr, rupnp::Error> {
        self.service
            .action(&self.device_url, "GetExternalIPAddress", "")
            .await?
            .get("NewExternalIPAddress")
            .ok_or(InvalidResponse(
                "response has no NewExternalIPAddress field",
            ))?
            .parse::<net::IpAddr>()
            .map_err(|_| InvalidResponse("failed to parse IP address").into())
    }

    fn mappings_snapshot(&self) -> Vec<MappingData> {
        self.mappings
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    }
}

enum State {
    Start,
    AddingPortMappingFirstStage,
    // Contains the wakeup time.
    SleepingFirstStage(DateTime<Local>),
    GettingIpAddress,
    AddingPortMappingSecondStage,
    // Contains the wakeup time.
    SleepingSecondStage(DateTime<Local>),
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
            Self::SleepingFirstStage(wake_up_time) => {
                write!(f, "SleepingFirstStage {}", wake_up_time.format("%T"))
            }
            Self::GettingIpAddress => {
                write!(f, "GettingIpAddress")
            }
            Self::AddingPortMappingSecondStage => {
                write!(f, "AddingPortMappingSecondStage")
            }
            Self::SleepingSecondStage(wake_up_time) => {
                write!(f, "SleepingSecondStage {}", wake_up_time.format("%T"))
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
