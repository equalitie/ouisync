use super::{
    channel_id::ChannelId,
    message::{AckData, Message},
    message_dispatcher::{ChannelClosed, UnreliableContentSink, UnreliableContentStream},
};
use async_trait::async_trait;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

pub(super) type ContentSink = ContentSinkT<UnreliableContentSink>;
pub(super) type ContentStream = ContentStreamT<UnreliableContentStream>;

pub(super) struct ContentStreamT<Stream> {
    shared: Arc<Shared>,
    unreliable_stream: Stream,
}

impl<Stream: UnreliableContentStreamTrait> ContentStreamT<Stream> {
    pub fn channel(&self) -> &ChannelId {
        &self.unreliable_stream.channel()
    }

    pub async fn recv(&mut self) -> Result<Vec<u8>, ChannelClosed> {
        Ok(self.unreliable_stream.recv().await?.content)
    }
}

#[derive(Clone)]
pub(super) struct ContentSinkT<Sink> {
    shared: Arc<Shared>,
    //next_seq_num: Arc::new(AtomicU64::new(0)),
    unreliable_sink: Sink,
}

impl<T: UnreliableContentSinkTrait> ContentSinkT<T> {
    /// Returns whether the send succeeded.
    pub async fn send(&self, content: Vec<u8>) -> Result<(), ChannelClosed> {
        let message = Message {
            seq_num: 0,
            ack_data: AckData::default(),
            channel: *self.channel(),
            content,
        };

        self.unreliable_sink.send(message).await
    }

    pub fn channel(&self) -> &ChannelId {
        self.unreliable_sink.channel()
    }
}

pub(super) fn new(
    unreliable_sink: UnreliableContentSink,
    unreliable_stream: UnreliableContentStream,
) -> (ContentSink, ContentStream) {
    let shared = Arc::new(Shared {});

    (
        ContentSinkT {
            shared: shared.clone(),
            unreliable_sink,
        },
        ContentStreamT {
            shared,
            unreliable_stream,
        },
    )
}

struct Shared {}

#[async_trait]
pub(super) trait UnreliableContentStreamTrait {
    async fn recv(&mut self) -> Result<Message, ChannelClosed>;
    fn channel(&self) -> &ChannelId;
}

#[async_trait]
pub(super) trait UnreliableContentSinkTrait {
    async fn send(&self, _: Message) -> Result<(), ChannelClosed>;
    fn channel(&self) -> &ChannelId;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    // --- Sink ---------------------------------------------------------------
    struct TestSink {
        tx: mpsc::Sender<Message>,
        channel: ChannelId,
    }

    #[async_trait]
    impl UnreliableContentSinkTrait for TestSink {
        async fn send(&self, message: Message) -> Result<(), ChannelClosed> {
            self.tx.send(message).await.map_err(|_| ChannelClosed)
        }

        fn channel(&self) -> &ChannelId {
            &self.channel
        }
    }

    // --- Stream --------------------------------------------------------------
    struct TestStream {
        rx: mpsc::Receiver<Message>,
        channel: ChannelId,
    }

    #[async_trait]
    impl UnreliableContentStreamTrait for TestStream {
        async fn recv(&mut self) -> Result<Message, ChannelClosed> {
            self.rx.recv().await.ok_or(ChannelClosed)
        }

        fn channel(&self) -> &ChannelId {
            &self.channel
        }
    }

    // -------------------------------------------------------------------------
    fn new_test_channel() -> (ContentSinkT<TestSink>, ContentStreamT<TestStream>) {
        let shared = Arc::new(Shared {});
        let (tx, rx) = mpsc::channel(1);

        (
            ContentSinkT {
                shared: shared.clone(),
                unreliable_sink: TestSink {
                    tx,
                    channel: ChannelId::default(),
                },
            },
            ContentStreamT {
                shared,
                unreliable_stream: TestStream {
                    rx,
                    channel: ChannelId::default(),
                },
            },
        )
    }
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn recv_on_stream() {
        let (tx, mut rx) = new_test_channel();

        tx.send(Vec::new()).await.unwrap();
        rx.recv().await.unwrap();
    }
}
