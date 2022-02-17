use crate::scoped_task::{self, ScopedJoinHandle};
use futures_util::TryStreamExt;
use futures_util::{Stream, StreamExt};
use rupnp::{
    http::Uri,
    ssdp::{SearchTarget, URN},
    Device, Service,
};
use std::{
    collections::HashMap,
    fmt,
    future::Future,
    io, net,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::sleep;

#[derive(Clone, Copy)]
pub(crate) struct Mapping {
    pub internal: u16,
    pub external: u16,
    pub protocol: Protocol,
}

#[derive(Clone, Copy)]
pub(crate) enum Protocol {
    Tcp,
    Udp,
}

// `Option` is used to keep track of which Uris have already been tried so as to not flood the
// debug log with repeated "device failure" warnings.
type JobHandles = HashMap<Uri, Option<ScopedJoinHandle<()>>>;

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Tcp => write!(f, "TCP"),
            Self::Udp => write!(f, "UDP"),
        }
    }
}

pub(crate) struct PortForwarder {
    _task: ScopedJoinHandle<()>,
}

impl PortForwarder {
    pub fn new<I>(mappings: I) -> Self
    where
        I: IntoIterator<Item = Mapping>,
    {
        let mappings: Vec<_> = mappings.into_iter().collect();
        let task = scoped_task::spawn(async move {
            for mapping in &mappings {
                log::info!(
                    "UPnP starting port forwarding EXT:{} -> INT:{} ({})",
                    mapping.external,
                    mapping.internal,
                    mapping.protocol,
                );
            }

            let result = Self::run(mappings).await;
            // Warning, because we don't actually expect this to happen.
            log::warn!("UPnP port forwarding ended ({:?})", result)
        });

        Self { _task: task }
    }

    async fn run(mappings: Vec<Mapping>) -> Result<(), rupnp::Error> {
        // Devices may have a timeout period when they don't respond to repeated queries, the
        // DISCOVERY_RUDATION constant should be higher than that. The rupnp project internally
        // assumes this duration is three seconds.
        const DISCOVERY_DURATION: Duration = Duration::from_secs(5);
        // SLEEP_DURATION is relevant e.g. when the user enables UPnP on their router. We don't
        // want them to have to wait too long to start syncing after they do so. But we also have
        // to balance it with not spamming the network with multicast queries.
        const SLEEP_DURATION: Duration = Duration::from_secs(15);

        let job_handles = Arc::new(Mutex::new(JobHandles::new()));

        // Make it an Arc so we can clone lazily (only when we're not already running a spawned
        // coroutine on an IGD device).
        let mappings = Arc::new(mappings);

        // Periodically check for new devices: maybe UPnP was enabled later after the app started,
        // maybe the device was swapped for another one,...
        loop {
            let mut device_urls = Box::pin(
                discover_device_urls(&SearchTarget::RootDevice, DISCOVERY_DURATION).await?,
            );

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
                Self::spawn_if_not_running(device_url.clone(), &job_handles, move || {
                    Box::pin(async move {
                        let device = Device::from_url(device_url).await?;

                        // We are only interested in IGD.
                        if device.device_type().typ() != "InternetGatewayDevice" {
                            return Ok(());
                        }

                        if let Some((service, version)) = find_connection_service(&device) {
                            let url = device.url().clone();

                            let per_igd_port_forwarder = PerIGDPortForwarder {
                                device_url: url.clone(),
                                service,
                                mappings: (*mappings).clone(),
                                _version: version,
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

enum Version {
    V1,
    V2,
}

struct PerIGDPortForwarder {
    device_url: Uri,
    service: Service,
    mappings: Vec<Mapping>,
    // Indicate whether we're talking to an IGDv1 or IGDv2 device. I though we might need it but
    // currently we don't because the few calls we do are common for both versions. But keeping it
    // here for posterity.
    _version: Version,
}

impl PerIGDPortForwarder {
    async fn run(&self) -> Result<(), rupnp::Error> {
        let local_ip = local_address_to(&self.device_url).await?;

        let lease_duration = Duration::from_secs(5 * 60);
        let sleep_delta = Duration::from_secs(5);
        let error_sleep_duration = Duration::from_secs(15);
        let sleep_duration = lease_duration.saturating_sub(sleep_delta);

        let mut error_reported = false;
        let mut ext_port_reported = false;
        let mut ext_addr_reported = false;

        loop {
            if self
                .add_port_mappings(&local_ip, lease_duration)
                .await
                .is_err()
            {
                sleep(error_sleep_duration).await;
                continue;
            }

            if !ext_port_reported {
                ext_port_reported = true;

                for mapping in &self.mappings {
                    log::info!(
                        "UPnP port forwarding started on external port {}:{}",
                        mapping.protocol,
                        mapping.external
                    );
                }
            }

            match self.get_external_ip_address().await {
                Ok(addr) => {
                    if !ext_addr_reported {
                        error_reported = false;
                        ext_addr_reported = true;
                        log::info!("UPnP the external IP address is {}", addr);
                    }
                }
                Err(e) => {
                    if !error_reported {
                        error_reported = true;
                        ext_addr_reported = false;
                        log::warn!("UPnP failed to retrieve external IP address: {:?}", e);
                    }
                }
            }

            sleep(sleep_duration).await;

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
            if self
                .add_port_mappings(&local_ip, lease_duration)
                .await
                .is_err()
            {
                sleep(error_sleep_duration).await;
                continue;
            }

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
    ) -> Result<(), rupnp::Error> {
        let lease_duration = if lease_duration == Duration::ZERO {
            Duration::ZERO
        } else {
            std::cmp::max(Duration::from_secs(1), lease_duration)
        };

        const MAPPING_DESCRIPTION: &str = "OuiSync";

        for mapping in &self.mappings {
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
}

fn find_versioned_connection_service(
    device: &rupnp::DeviceSpec,
    version: u32,
) -> Option<&rupnp::Service> {
    const SCHEMA: &str = "schemas-upnp-org";

    device
        .find_device(&URN::device(SCHEMA, "WANDevice", version))
        .and_then(|device| device.find_device(&URN::device(SCHEMA, "WANConnectionDevice", version)))
        .and_then(|device| device.find_service(&URN::service(SCHEMA, "WANIPConnection", version)))
}

fn find_connection_service(device: &rupnp::DeviceSpec) -> Option<(rupnp::Service, Version)> {
    find_versioned_connection_service(device, 2)
        .map(|service| (service.clone(), Version::V2))
        .or_else(|| {
            find_versioned_connection_service(device, 1)
                .map(|service| (service.clone(), Version::V1))
        })
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
