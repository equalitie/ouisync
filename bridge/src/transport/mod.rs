pub mod foreign;
pub mod local;
pub mod native;
pub mod remote;
pub mod socket;

use std::sync::Arc;

use crate::{
    error::Result,
    protocol::{self, Request, Response},
    state::{ClientState, ServerState},
};
use async_trait::async_trait;

#[async_trait(?Send)]
pub trait Client {
    async fn invoke(&self, request: Request) -> Result<Response>;
    async fn close(&self) {}
}

#[async_trait]
pub trait Handler: Clone + Send + Sync + 'static {
    async fn handle(&self, client_state: &ClientState, request: Request) -> Result<Response>;
}

#[derive(Clone)]
pub struct DefaultHandler {
    server_state: Arc<ServerState>,
}

impl DefaultHandler {
    pub fn new(server_state: Arc<ServerState>) -> Self {
        Self { server_state }
    }
}

#[async_trait]
impl Handler for DefaultHandler {
    async fn handle(&self, client_state: &ClientState, request: Request) -> Result<Response> {
        protocol::dispatch(&self.server_state, client_state, request).await
    }
}
