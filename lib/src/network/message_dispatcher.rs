//! Utilities for sending and receiving messages across the network.

use super::{
    stats::{ByteCounters, Instrumented},
    throttle::{Throttle, ThrottledBusRecvStream, ThrottledBusSendStream},
};
use net::{
    bus::{Bus, TopicId},
    unified::Connection,
};
use std::sync::Arc;
use tokio_util::codec::{length_delimited, FramedRead, FramedWrite, LengthDelimitedCodec};

/// Reads/writes messages from/to the underlying TCP or QUIC connection and dispatches them to
/// individual streams/sinks based on their topic ids (in the MessageDispatcher's and
/// MessageBroker's contexts, there is a one-to-one relationship between the topic id and a
/// repository id).
#[derive(Clone)]
pub(super) struct MessageDispatcher {
    bus: net::bus::Bus,
    total_counters: Arc<ByteCounters>,
    peer_counters: Arc<ByteCounters>,
    throttle: Throttle,
}

impl MessageDispatcher {
    pub fn builder(connection: Connection) -> Builder {
        Builder {
            connection,
            total_counters: None,
            peer_counters: None,
            throttle: None,
        }
    }

    /// Opens a sink and a stream for communication on the given topic.
    pub fn open(
        &self,
        topic_id: TopicId,
        repo_counters: Arc<ByteCounters>,
    ) -> (MessageSink, MessageStream) {
        let (writer, reader) = self.bus.create_topic(topic_id);

        let writer = self.throttle.limit_writer(writer);
        let reader = self.throttle.limit_reader(reader);

        let writer = Instrumented::new(writer, self.total_counters.clone());
        let writer = Instrumented::new(writer, self.peer_counters.clone());
        let writer = Instrumented::new(writer, repo_counters.clone());

        let reader = Instrumented::new(reader, self.total_counters.clone());
        let reader = Instrumented::new(reader, self.peer_counters.clone());
        let reader = Instrumented::new(reader, repo_counters);

        let codec = make_codec();

        let sink = FramedWrite::new(writer, codec.clone());
        let stream = FramedRead::new(reader, codec);

        (sink, stream)
    }

    /// Gracefully shuts down this dispatcher. This closes the underlying connection and all open
    /// message streams and sinks.
    ///
    /// Note: the dispatcher also shuts down automatically when it's been dropped. Calling this
    /// function is still useful when one wants to force the existing streams/sinks to close and/or
    /// to wait until the shutdown has been completed.
    pub async fn shutdown(self) {
        self.bus.close().await;
    }
}

pub(super) struct Builder {
    connection: Connection,
    total_counters: Option<Arc<ByteCounters>>,
    peer_counters: Option<Arc<ByteCounters>>,
    throttle: Option<Throttle>,
}

impl Builder {
    pub fn with_total_counters(self, counters: Arc<ByteCounters>) -> Self {
        Self {
            total_counters: Some(counters),
            ..self
        }
    }

    pub fn with_peer_counters(self, counters: Arc<ByteCounters>) -> Self {
        Self {
            peer_counters: Some(counters),
            ..self
        }
    }

    pub fn with_throttle(self, throttle: Throttle) -> Self {
        Self {
            throttle: Some(throttle),
            ..self
        }
    }

    pub fn build(self) -> MessageDispatcher {
        MessageDispatcher {
            bus: Bus::new(self.connection),
            total_counters: self.total_counters.unwrap_or_default(),
            peer_counters: self.peer_counters.unwrap_or_default(),
            throttle: self.throttle.unwrap_or_else(Throttle::new_no_limits),
        }
    }
}

fn make_codec() -> LengthDelimitedCodec {
    length_delimited::Builder::new()
        .big_endian()
        .length_field_type::<u16>()
        .new_codec()
}

// The streams/sinks are tripple-instrumented: once to collect the total cummulative traffic across
// all peers, once to collect the traffic per peer and once to collect the traffic per repo.
pub(super) type MessageStream = FramedRead<
    Instrumented<Instrumented<Instrumented<ThrottledBusRecvStream>>>,
    LengthDelimitedCodec,
>;

pub(super) type MessageSink = FramedWrite<
    Instrumented<Instrumented<Instrumented<ThrottledBusSendStream>>>,
    LengthDelimitedCodec,
>;

/// Create pair of Connections connected to each other. For tests only.
#[cfg(test)]
pub(super) async fn create_connection_pair() -> (Connection, Connection) {
    use futures_util::future;
    use net::{
        unified::{Acceptor, Connector},
        SocketOptions,
    };
    use std::net::Ipv4Addr;

    // NOTE: Make sure to keep the `reuse_addr` option disabled here to avoid one test to
    // accidentally connect to a different test (even from a different process). More details
    // here: https://gavv.net/articles/ephemeral-port-reuse/.

    let client = net::quic::configure((Ipv4Addr::LOCALHOST, 0).into(), SocketOptions::default())
        .unwrap()
        .0;
    let server = net::quic::configure((Ipv4Addr::LOCALHOST, 0).into(), SocketOptions::default())
        .unwrap()
        .1;

    let client = Connector::from(client);
    let server = Acceptor::from(server);

    let client = client.connect(*server.local_addr());
    let server = async { server.accept().await?.await };

    future::try_join(client, server).await.unwrap()
}

#[cfg(test)]
mod tests {
    use super::{super::stats::ByteCounters, *};
    use bytes::Bytes;
    use futures_util::SinkExt;
    use tokio_stream::StreamExt;

    #[tokio::test]
    async fn sanity_check() {
        crate::test_utils::init_log();

        let topic_id = TopicId::random();
        let send_content = b"hello world";

        let (client, server) = create_connection_pair().await;

        let server_dispatcher = MessageDispatcher::builder(server).build();

        let (_server_sink, mut server_stream) =
            server_dispatcher.open(topic_id, Arc::new(ByteCounters::default()));

        let client_dispatcher = MessageDispatcher::builder(client).build();

        let (mut client_sink, _client_stream) =
            client_dispatcher.open(topic_id, Arc::new(ByteCounters::default()));

        client_sink
            .send(Bytes::from_static(send_content))
            .await
            .unwrap();

        let recv_content = server_stream.try_next().await.unwrap().unwrap();
        assert_eq!(recv_content.as_ref(), send_content.as_ref());
    }
}
