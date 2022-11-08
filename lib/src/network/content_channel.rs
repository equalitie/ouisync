use super::{
    channel_id::ChannelId,
    message_dispatcher::{ChannelClosed, UnreliableContentSink, UnreliableContentStream}
};

pub(super) struct ContentStream {
    unreliable_stream: UnreliableContentStream,
}

impl ContentStream {
    pub fn new(unreliable_stream: UnreliableContentStream) -> Self {
        Self {
            unreliable_stream,
        }
    }

    pub fn channel(&self) -> &ChannelId {
        &self.unreliable_stream.channel()
    }

    pub async fn recv(&mut self) -> Result<Vec<u8>, ChannelClosed> {
        Ok(self.unreliable_stream.recv().await?.content)
    }
}

#[derive(Clone)]
pub(super) struct ContentSink {
    unreliable_sink: UnreliableContentSink,
}

impl ContentSink {
    pub fn new(unreliable_sink: UnreliableContentSink) -> Self {
        Self {
            unreliable_sink
        }
    }

    /// Returns whether the send succeeded.
    pub async fn send(&self, content: Vec<u8>) -> Result<(), ChannelClosed> {
        self.unreliable_sink.send(content).await
    }

    pub fn channel(&self) -> &ChannelId {
        self.unreliable_sink.channel()
    }
}

