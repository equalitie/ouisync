//! Client and Server than run in the same process and are written in the same language.
//! Note that only the Client is actually implemented because it invokes the operations directly
//! and doesn't need a separate server component.

use super::Client;
use crate::{
    error::Result,
    protocol::{self, Request, Response},
    state::{ClientState, ServerState},
};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct NativeClient {
    server_state: Arc<ServerState>,
    client_state: ClientState,
}

impl NativeClient {
    pub fn new(server_state: Arc<ServerState>) -> Self {
        // TODO: support notifications
        let (notification_tx, _notification_rx) = mpsc::channel(1);
        let client_state = ClientState { notification_tx };

        Self {
            server_state,
            client_state,
        }
    }
}

#[async_trait(?Send)]
impl Client for NativeClient {
    async fn invoke(&self, request: Request) -> Result<Response> {
        protocol::dispatch(&self.server_state, &self.client_state, request).await
    }

    async fn close(&self) {
        self.server_state.close().await;
    }
}
