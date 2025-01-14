use crate::udp::DatagramSocket;
use bytecodec::{DecodeExt, EncodeExt};
use std::{
    collections::{hash_map::Entry, HashMap},
    fmt, io, mem,
    net::SocketAddr,
    sync::Mutex,
    time::Duration,
};
use stun_codec::{
    convert::TryAsRef,
    rfc5389::{attributes::XorMappedAddress, methods::BINDING},
    rfc5780::attributes::{ChangeRequest, OtherAddress},
    Message, MessageClass, MessageDecoder, MessageEncoder, Method, TransactionId,
};
use tokio::{select, sync::Notify, time};

stun_codec::define_attribute_enums! {
    Attribute, AttributeDecoder, AttributeEncoder,
    [
        // RFC 5389
        XorMappedAddress,

        // RFC 5780
        ChangeRequest, OtherAddress
    ]
}

const TIMEOUT: Duration = Duration::from_secs(10);

/// [STUN](https://en.wikipedia.org/wiki/STUN) client
pub struct StunClient<T: DatagramSocket> {
    socket: T,
    responses: Mutex<HashMap<TransactionId, ResponseSlot>>,
    responses_notify: Notify,
}

impl<T: DatagramSocket> StunClient<T> {
    pub fn new(socket: T) -> Self {
        Self {
            socket,
            responses: Mutex::default(),
            responses_notify: Notify::new(),
        }
    }

    /// Query our external address as seen by the given STUN server.
    pub async fn external_addr(&self, server_addr: SocketAddr) -> io::Result<SocketAddr> {
        let request = make_request(BINDING);
        let response = self.send_request(server_addr, request).await?;

        get_xor_mapped_address(&response)
    }

    /// Determine NAT mapping behavior
    /// (RFC 5780, section 4.3, https://datatracker.ietf.org/doc/html/rfc5780#section-4.3)
    pub async fn nat_mapping(&self, server_addr: SocketAddr) -> io::Result<NatBehavior> {
        // 4.3.  Determining NAT Mapping Behavior
        //
        //    This will require at most three tests.  In test I, the client
        //    performs the UDP connectivity test.  The server will return its
        //    alternate address and port in OTHER-ADDRESS in the binding response.
        //    If OTHER-ADDRESS is not returned, the server does not support this
        //    usage and this test cannot be run.  The client examines the XOR-
        //    MAPPED-ADDRESS attribute.  If this address and port are the same as
        //    the local IP address and port of the socket used to send the request,
        //    the client knows that it is not NATed and the effective mapping will
        //    be Endpoint-Independent.
        //
        //    In test II, the client sends a Binding Request to the alternate
        //    address, but primary port.  If the XOR-MAPPED-ADDRESS in the Binding
        //    Response is the same as test I the NAT currently has Endpoint-
        //    Independent Mapping.  If not, test III is performed: the client sends
        //    a Binding Request to the alternate address and port.  If the XOR-
        //    MAPPED-ADDRESS matches test II, the NAT currently has Address-
        //    Dependent Mapping; if it doesn't match it currently has Address and
        //    Port-Dependent Mapping.
        let local_addr = self.socket.local_addr()?;

        // test I
        let request = make_request(BINDING);
        let response = self.send_request(server_addr, request).await?;
        let mapped_addr_1 = get_xor_mapped_address(&response)?;
        let other_addr = get_other_address(&response)?;

        if mapped_addr_1 == local_addr {
            Ok(NatBehavior::EndpointIndependent)
        } else {
            // RFC 5780, section 7.4, https://datatracker.ietf.org/doc/html/rfc5780#section-7.4
            //   OTHER-ADDRESS MUST NOT be inserted into a Binding Response unless the
            //   server has a second IP address.
            //
            // But some servers do respond with the alternate IP address same as their own. In
            // those cases the below mapped_addr_2 would always be equal to mapped_addr_1 and we'd
            // return Ok(NatBehavior::EndpointIndependent) regardless of what the actual NAT
            // mapping is.

            if other_addr.ip() == server_addr.ip() {
                return Err(io::Error::new(io::ErrorKind::Other, "Faulty STUN server"));
            }

            // test II
            let request = make_request(BINDING);
            let dst_addr = SocketAddr::new(other_addr.ip(), server_addr.port());
            let response = self.send_request(dst_addr, request).await?;
            let mapped_addr_2 = get_xor_mapped_address(&response)?;

            if mapped_addr_2 == mapped_addr_1 {
                Ok(NatBehavior::EndpointIndependent)
            } else {
                // test III
                let request = make_request(BINDING);
                let response = self.send_request(other_addr, request).await?;
                let mapped_addr_3 = get_xor_mapped_address(&response)?;

                if mapped_addr_3 == mapped_addr_2 {
                    Ok(NatBehavior::AddressDependent)
                } else {
                    Ok(NatBehavior::AddressAndPortDependent)
                }
            }
        }
    }

    /// Determine NAT filtering behavior
    /// (RFC 5780, section 4.4, https://datatracker.ietf.org/doc/html/rfc5780#section-4.4)
    pub async fn nat_filtering(&self, server_addr: SocketAddr) -> io::Result<NatBehavior> {
        // 4.4.  Determining NAT Filtering Behavior
        //
        //    This will also require at most three tests.  These tests are
        //    sensitive to prior state on the NAT.
        //
        //    In test I, the client performs the UDP connectivity test.  The server
        //    will return its alternate address and port in OTHER-ADDRESS in the
        //    binding response.  If OTHER-ADDRESS is not returned, the server does
        //    not support this usage and this test cannot be run.
        //
        //    In test II, the client sends a binding request to the primary address
        //    of the server with the CHANGE-REQUEST attribute set to change-port
        //    and change-IP.  This will cause the server to send its response from
        //    its alternate IP address and alternate port.  If the client receives
        //    a response, the current behavior of the NAT is Endpoint-Independent
        //    Filtering.
        //
        //    If no response is received, test III must be performed to distinguish
        //    between Address-Dependent Filtering and Address and Port-Dependent
        //    Filtering.  In test III, the client sends a binding request to the
        //    original server address with CHANGE-REQUEST set to change-port.  If
        //    the client receives a response, the current behavior is Address-
        //    Dependent Filtering; if no response is received, the current behavior
        //    is Address and Port-Dependent Filtering.

        // test I
        let request = make_request(BINDING);
        let response = self.send_request(server_addr, request).await?;
        // Only to check that the server suports OTHER-ADDRESS:
        let _other_addr = get_other_address(&response)?;

        // test II
        let mut request = make_request(BINDING);
        request.add_attribute(ChangeRequest::new(true, true));

        let response = match self.send_request(server_addr, request).await {
            Ok(response) => Some(response),
            Err(error) if error.kind() == io::ErrorKind::TimedOut => None,
            Err(error) => return Err(error),
        };

        if response.is_some() {
            Ok(NatBehavior::EndpointIndependent)
        } else {
            // test III
            let mut request = make_request(BINDING);
            request.add_attribute(ChangeRequest::new(false, true));

            let response = match self.send_request(server_addr, request).await {
                Ok(response) => Some(response),
                Err(error) if error.kind() == io::ErrorKind::TimedOut => None,
                Err(error) => return Err(error),
            };

            if response.is_some() {
                Ok(NatBehavior::AddressDependent)
            } else {
                Ok(NatBehavior::AddressAndPortDependent)
            }
        }
    }

    /// Gets a reference to the underlying socket.
    pub fn get_ref(&self) -> &T {
        &self.socket
    }

    async fn send_request(
        &self,
        server_addr: SocketAddr,
        message: Message<Attribute>,
    ) -> io::Result<Message<Attribute>> {
        let transaction_id = message.transaction_id();
        let _guard = self.expect_response(transaction_id);

        self.send(server_addr, message).await?;

        let result = time::timeout(TIMEOUT, async move {
            loop {
                // Need to obtain the `notified` future before removing the response but await it
                // after. This is so we don't miss any notifications.
                let notified = self.responses_notify.notified();

                if let Some(response) = self.remove_response(transaction_id) {
                    return Ok(response);
                }

                select! {
                    response = self.recv() => {
                        let response = response?;

                        if response.transaction_id() == transaction_id {
                            return Ok(response);
                        } else {
                            self.insert_response(response);
                        }
                    },
                    _ = notified => (),
                }
            }
        })
        .await;

        match result {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "request timed out")),
        }
    }

    async fn send(&self, server_addr: SocketAddr, message: Message<Attribute>) -> io::Result<()> {
        let mut encoder = MessageEncoder::new();
        let buffer = encoder
            .encode_into_bytes(message)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        self.socket.send_to(&buffer, server_addr).await?;

        Ok(())
    }

    async fn recv(&self) -> io::Result<Message<Attribute>> {
        let mut decoder = MessageDecoder::new();
        let mut buffer = vec![0; 512];

        loop {
            let (len, _) = self.socket.recv_from(&mut buffer[..]).await?;

            buffer.resize(len, 0);

            // Decode failure here is probably caused by receiving a packet from another protocol
            // multiplexed on the same socket. Ignore and try again.
            let response = match decoder.decode_from_bytes(&buffer[..]) {
                Ok(Ok(response)) => response,
                Ok(Err(_)) | Err(_) => continue,
            };

            return Ok(response);
        }
    }

    fn expect_response(&self, transaction_id: TransactionId) -> RemoveGuard<'_> {
        self.responses
            .lock()
            .unwrap()
            .insert(transaction_id, ResponseSlot::Pending);

        RemoveGuard {
            responses: &self.responses,
            transaction_id,
        }
    }

    fn insert_response(&self, response: Message<Attribute>) {
        let mut responses = self.responses.lock().unwrap();

        let Some(slot) = responses.get_mut(&response.transaction_id()) else {
            return;
        };

        *slot = ResponseSlot::Received(response);
        self.responses_notify.notify_waiters();
    }

    fn remove_response(&self, transaction_id: TransactionId) -> Option<Message<Attribute>> {
        match self.responses.lock().unwrap().entry(transaction_id) {
            Entry::Occupied(mut entry) => {
                match mem::replace(entry.get_mut(), ResponseSlot::Pending) {
                    ResponseSlot::Received(message) => {
                        entry.remove();
                        Some(message)
                    }
                    ResponseSlot::Pending => None,
                }
            }
            Entry::Vacant(_) => None,
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum NatBehavior {
    EndpointIndependent,
    AddressDependent,
    AddressAndPortDependent,
}

impl fmt::Display for NatBehavior {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::EndpointIndependent => write!(f, "endpoint independent"),
            Self::AddressDependent => write!(f, "address dependent"),
            Self::AddressAndPortDependent => write!(f, "address and port dependent"),
        }
    }
}

enum ResponseSlot {
    Pending,
    Received(Message<Attribute>),
}

struct RemoveGuard<'a> {
    responses: &'a Mutex<HashMap<TransactionId, ResponseSlot>>,
    transaction_id: TransactionId,
}

impl Drop for RemoveGuard<'_> {
    fn drop(&mut self) {
        self.responses.lock().unwrap().remove(&self.transaction_id);
    }
}

fn make_request(method: Method) -> Message<Attribute> {
    Message::new(
        MessageClass::Request,
        method,
        TransactionId::new(rand::random()),
    )
}

fn get_attribute<'a, A>(message: &'a Message<Attribute>, name: &'static str) -> io::Result<&'a A>
where
    A: stun_codec::Attribute,
    Attribute: TryAsRef<A>,
{
    message.get_attribute::<A>().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            format!("server does not support {name}"),
        )
    })
}

fn get_xor_mapped_address(message: &Message<Attribute>) -> io::Result<SocketAddr> {
    get_attribute::<XorMappedAddress>(message, "XOR-MAPPED-ADDRESS").map(|attr| attr.address())
}

fn get_other_address(message: &Message<Attribute>) -> io::Result<SocketAddr> {
    get_attribute::<OtherAddress>(message, "OTHER-ADDRESS").map(|attr| attr.address())
}
