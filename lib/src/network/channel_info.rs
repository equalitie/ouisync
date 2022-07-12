use super::{message::MessageChannel, runtime_id::PublicRuntimeId};
use std::{fmt, future::Future};
use tokio::task_local;

task_local! {
    static CURRENT: ChannelInfo;
}

#[derive(Clone, Copy)]
pub(super) struct ChannelInfo {
    channel: MessageChannel,
    this_runtime_id: PublicRuntimeId,
    that_runtime_id: PublicRuntimeId,
}

impl ChannelInfo {
    pub fn new(
        channel: MessageChannel,
        this_runtime_id: PublicRuntimeId,
        that_runtime_id: PublicRuntimeId,
    ) -> Self {
        Self {
            channel,
            this_runtime_id,
            that_runtime_id,
        }
    }

    pub fn current() -> Self {
        CURRENT.get()
    }

    pub async fn apply<F>(self, fut: F) -> F::Output
    where
        F: Future,
    {
        CURRENT.scope(self, fut).await
    }
}

impl fmt::Display for ChannelInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "[{:?} -> {:?} ({:?})]",
            self.this_runtime_id.as_public_key(),
            self.that_runtime_id.as_public_key(),
            self.channel,
        )
    }
}
