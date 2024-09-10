use super::TopicId;
use crate::unified::{Connection, RecvStream, SendStream};
use slab::Slab;
use std::{io, sync::Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    select,
    sync::oneshot,
};
use tracing::instrument;

/// Wrapper around `Connection` that allow to establish incoming and outgoing streams that are bound
/// to a given topic. When the two connected peers establish streams bound to the same topic
/// (one peer incoming and one peer outgoing), those streams will be connected and will allow
/// conmmunication between them.
///
/// NOTE: incoming streams wait for the corresponding outgoing streams but not the other way around.
/// That is, if one peer calls `incoming`, it will wait until the other peer calls `outgoing` with
/// the same topic. However, if one peer calls `outgoing` without the other calling `incoming`
/// with the same topic, the returned stream will be immediatelly closed.
pub(super) struct Dispatcher {
    connection: Connection,
    registry: Mutex<Registry>,
}

impl Dispatcher {
    pub fn new(connection: Connection) -> Self {
        Self {
            connection,
            registry: Mutex::new(Registry::default()),
        }
    }

    /// Establish an incoming stream bound to the given topic.
    #[instrument(skip_all)]
    pub async fn incoming(&self, topic_id: TopicId) -> io::Result<(SendStream, RecvStream)> {
        let (reply_tx, reply_rx) = oneshot::channel();

        let key = self.registry.lock().unwrap().insert((topic_id, reply_tx));
        let guard = CancelGuard::arm(&self.registry, key);

        let accept = async {
            loop {
                let (send_stream, mut recv_stream) = match self.connection.incoming().await {
                    Ok(streams) => streams,
                    Err(error) => {
                        tracing::debug!(?error, "failed to accept incoming stream");
                        return Err(io::Error::other(error));
                    }
                };

                let mut buffer = [0; TopicId::SIZE];
                let topic_id = match recv_stream.read_exact(&mut buffer).await {
                    Ok(_) => TopicId::from(buffer),
                    Err(error) => {
                        tracing::debug!(?error, "failed to read topic id from incoming stream");
                        continue;
                    }
                };

                let reply_tx = {
                    let mut registry = self.registry.lock().unwrap();
                    let key = registry
                        .iter()
                        .find(|(_, (registry_topic_id, _))| *registry_topic_id == topic_id)
                        .map(|(key, _)| key);

                    key.map(|key| registry.remove(key).1)
                };

                if let Some(reply_tx) = reply_tx {
                    reply_tx.send((send_stream, recv_stream)).ok();
                } else {
                    tracing::debug!(?topic_id, "unsolicited incoming stream");
                    continue;
                }
            }
        };

        select! {
            result = accept => result,
            result = reply_rx => {
                guard.disarm();

                // unwrap is OK because the associated `reply_tx` is only dropped after we send on
                // it.
                Ok(result.unwrap())
            }
        }
    }

    /// Establish an outgoing stream bound to the given topic.
    #[instrument(skip_all)]
    pub async fn outgoing(&self, topic_id: TopicId) -> io::Result<(SendStream, RecvStream)> {
        let (mut send_stream, recv_stream) = self
            .connection
            .outgoing()
            .await
            .inspect_err(|error| tracing::error!(?error))
            .map_err(io::Error::other)?;

        send_stream
            .write_all(topic_id.as_bytes())
            .await
            .inspect_err(|error| tracing::error!(?error))?;

        Ok((send_stream, recv_stream))
    }

    pub async fn close(&self) {
        self.connection.close().await
    }
}

type Registry = Slab<(TopicId, oneshot::Sender<(SendStream, RecvStream)>)>;

struct CancelGuard<'a> {
    armed: bool,
    key: usize,
    registry: &'a Mutex<Registry>,
}

impl<'a> CancelGuard<'a> {
    fn arm(registry: &'a Mutex<Registry>, key: usize) -> Self {
        Self {
            armed: true,
            registry,
            key,
        }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for CancelGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        self.registry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .try_remove(self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{create_connected_connections, create_connected_peers, Proto};
    use assert_matches::assert_matches;
    use futures_util::future;

    #[tokio::test]
    async fn sanity_check_tcp() {
        sanity_check_case(Proto::Tcp).await
    }

    #[tokio::test]
    async fn sanity_check_quic() {
        sanity_check_case(Proto::Quic).await
    }

    async fn sanity_check_case(proto: Proto) {
        let (client, server) = create_connected_peers(proto);
        let (client, server) = create_connected_connections(&client, &server).await;

        let client = Dispatcher::new(client);
        let server = Dispatcher::new(server);

        let topic_id = TopicId::random();

        let (
            (mut client_send_stream, mut client_recv_stream),
            (mut server_send_stream, mut server_recv_stream),
        ) = future::try_join(client.outgoing(topic_id), server.incoming(topic_id))
            .await
            .unwrap();

        let client_message = b"hello from client";
        let server_message = b"hello from server";

        client_send_stream.write_all(client_message).await.unwrap();

        let mut buffer = [0; 17];
        server_recv_stream.read_exact(&mut buffer).await.unwrap();
        assert_eq!(&buffer, client_message);

        server_send_stream.write_all(server_message).await.unwrap();

        let mut buffer = [0; 17];
        client_recv_stream.read_exact(&mut buffer).await.unwrap();
        assert_eq!(&buffer, server_message);
    }

    #[tokio::test]
    async fn unsolicited_stream() {
        let (client, server) = create_connected_peers(Proto::Quic);
        let (client, server) = create_connected_connections(&client, &server).await;

        let client = Dispatcher::new(client);
        let server = Dispatcher::new(server);

        let client_task = async {
            let (mut send_stream, mut recv_stream) =
                client.outgoing(TopicId::random()).await.unwrap();

            send_stream.write_all(b"ping").await.unwrap();

            let mut buffer = [0; 1];
            assert_matches!(
                recv_stream.read_exact(&mut buffer).await,
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof
            );
        };

        let server_task = async {
            server.incoming(TopicId::random()).await.ok();
            unreachable!();
        };

        select! {
            _ = client_task => (),
            _ = server_task => (),
        };
    }
}
