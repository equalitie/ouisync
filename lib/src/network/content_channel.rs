use super::{
    channel_id::ChannelId,
    message_dispatcher::{ChannelClosed, UnreliableContentSink, UnreliableContentStream},
    message::Message,
};
use std::sync::Arc;
use async_trait::async_trait;

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
    unreliable_sink: Sink,
}

impl<T: UnreliableContentSinkTrait> ContentSinkT<T> {
    /// Returns whether the send succeeded.
    pub async fn send(&self, content: Vec<u8>) -> Result<(), ChannelClosed> {
        self.unreliable_sink.send(content).await
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
    async fn send(&self, content: Vec<u8>) -> Result<(), ChannelClosed>;
    fn channel(&self) -> &ChannelId;
}
