use crate::udp::DatagramSocket;
use bytecodec::{DecodeExt, EncodeExt};
use std::{
    collections::{hash_map::Entry, HashMap},
    io, mem,
    net::SocketAddr,
    sync::Mutex,
};
use stun_codec::{
    rfc5389::{attributes::XorMappedAddress, methods::BINDING, Attribute},
    Message, MessageClass, MessageDecoder, MessageEncoder, TransactionId,
};
use tokio::{select, sync::Notify};

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
        let request = Message::new(
            MessageClass::Request,
            BINDING,
            TransactionId::new(rand::random()),
        );
        let response = self.send_request(server_addr, request).await?;

        return response
            .get_attribute::<XorMappedAddress>()
            .map(|attr| attr.address())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no address in response"));
    }

    // TODO: NAT detection

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

        loop {
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
                _ = self.responses_notify.notified() => (),
            }
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
