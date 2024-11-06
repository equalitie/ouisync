//! Client and Server than run in the same process and are written in the same language.
//! Note that only the Client is actually implemented because it invokes the operations directly
//! and doesn't need a separate server component.

use crate::{
    handler::local::LocalHandler,
    protocol::{ProtocolError, Request, Response},
};
use ouisync_bridge::{
    protocol::SessionCookie,
    transport::{Handler as _, SessionContext},
};
use tokio::sync::mpsc;

pub(crate) struct NativeClient {
    handler: LocalHandler,
    context: SessionContext,
}

impl NativeClient {
    pub fn new(handler: LocalHandler) -> Self {
        let (notification_tx, _notification_rx) = mpsc::channel(1);
        let context = SessionContext {
            notification_tx,
            session_cookie: SessionCookie::DUMMY,
        };

        Self { handler, context }
    }

    pub async fn invoke(&self, request: Request) -> Result<Response, ProtocolError> {
        self.handler.handle(request, &self.context).await
    }

    pub async fn close(&self) {
        self.handler.close().await;
    }
}
