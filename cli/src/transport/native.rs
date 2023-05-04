//! Client and Server than run in the same process and are written in the same language.
//! Note that only the Client is actually implemented because it invokes the operations directly
//! and doesn't need a separate server component.

use crate::{
    handler::LocalHandler,
    protocol::{Request, Response},
};
use async_trait::async_trait;
use ouisync_bridge::{
    error::Result,
    transport::{Client, Handler as _, NotificationSender},
};
use tokio::sync::mpsc;

pub(crate) struct NativeClient {
    handler: LocalHandler,
    notification_tx: NotificationSender,
}

impl NativeClient {
    pub fn new(handler: LocalHandler) -> Self {
        let (notification_tx, _notification_rx) = mpsc::channel(1);

        Self {
            handler,
            notification_tx,
        }
    }
}

#[async_trait(?Send)]
impl Client for NativeClient {
    type Request = Request;
    type Response = Response;

    async fn invoke(&self, request: Self::Request) -> Result<Self::Response> {
        self.handler.handle(request, &self.notification_tx).await
    }

    async fn close(&self) {
        self.handler.close().await;
    }
}
