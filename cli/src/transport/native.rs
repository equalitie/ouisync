//! Client and Server than run in the same process and are written in the same language.
//! Note that only the Client is actually implemented because it invokes the operations directly
//! and doesn't need a separate server component.

use crate::{
    handler::local::LocalHandler,
    protocol::{Request, Response},
};
use ouisync_bridge::{
    error::Result,
    transport::{Handler as _, NotificationSender},
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

    pub async fn invoke(&self, request: Request) -> Result<Response> {
        self.handler.handle(request, &self.notification_tx).await
    }

    pub async fn close(&self) {
        self.handler.close().await;
    }
}
