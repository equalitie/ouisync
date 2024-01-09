use crate::udp::DatagramSocket;
use bytecodec::{DecodeExt, EncodeExt};
use std::{io, net::SocketAddr};
use stun_codec::{
    rfc5389::{self, attributes::XorMappedAddress},
    Attribute, Message, MessageClass, MessageDecoder, MessageEncoder, TransactionId,
};

/// [STUN](https://en.wikipedia.org/wiki/STUN) client
pub struct StunClient<T: DatagramSocket> {
    socket: T,
}

impl<T: DatagramSocket> StunClient<T> {
    pub fn new(socket: T) -> Self {
        Self { socket }
    }

    /// Query our external address as seen by the given STUN server.
    pub async fn external_addr(&self, server_addr: SocketAddr) -> io::Result<SocketAddr> {
        let tx_id = TransactionId::new(rand::random());
        let request = Message::<rfc5389::Attribute>::new(
            MessageClass::Request,
            rfc5389::methods::BINDING,
            tx_id,
        );

        send(&self.socket, server_addr, request).await?;

        let response: Message<rfc5389::Attribute> = recv(&self.socket, server_addr, tx_id).await?;

        return response
            .get_attribute::<XorMappedAddress>()
            .map(|attr| attr.address())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no address in response"));
    }

    // TODO: NAT detection
}

async fn send<S: DatagramSocket, A: Attribute>(
    socket: &S,
    server_addr: SocketAddr,
    message: Message<A>,
) -> io::Result<()> {
    let mut encoder = MessageEncoder::new();

    let buffer = encoder
        .encode_into_bytes(message)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    socket.send_to(&buffer, server_addr).await?;

    Ok(())
}

async fn recv<S: DatagramSocket, A: Attribute>(
    socket: &S,
    server_addr: SocketAddr,
    expected_transaction_id: TransactionId,
) -> io::Result<Message<A>> {
    let mut decoder = MessageDecoder::new();
    let mut buffer = vec![0; 512];

    // The socket might be multiplexed with other protocols so the receive response might
    // not be for us. In that case, try again.
    loop {
        let (len, recv_addr) = socket.recv_from(&mut buffer[..]).await?;

        if recv_addr != server_addr {
            continue;
        }

        buffer.resize(len, 0);

        // TODO: The `BrokenMessage` error might be also caused by the multiplexing issue,
        // consider retrying as well.
        let response = decoder
            .decode_from_bytes(&buffer[..])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "broken response"))?;

        if response.transaction_id() == expected_transaction_id {
            return Ok(response);
        } else {
            continue;
        }
    }
}
