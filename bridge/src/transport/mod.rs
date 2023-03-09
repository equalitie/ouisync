pub mod foreign;
pub mod socket;

use std::sync::Arc;

use crate::{
    error::Result,
    protocol::{self, Notification, Request, Response},
    state::State,
};
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::mpsc;

#[async_trait(?Send)]
pub trait Client {
    type Request;
    type Response;

    async fn invoke(&self, request: Self::Request) -> Result<Self::Response>;
    async fn close(&self) {}
}

#[async_trait]
pub trait Handler: Clone + Send + Sync + 'static {
    type Request: DeserializeOwned + Send;
    type Response: Serialize + Send;

    async fn handle(
        &self,
        request: Self::Request,
        notification_tx: &NotificationSender,
    ) -> Result<Self::Response>;
}

pub type NotificationSender = mpsc::Sender<(u64, Notification)>;

#[derive(Clone)]
pub struct DefaultHandler {
    state: Arc<State>,
}

impl DefaultHandler {
    pub fn new(state: Arc<State>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Handler for DefaultHandler {
    type Request = Request;
    type Response = Response;

    async fn handle(
        &self,
        request: Self::Request,
        notification_tx: &NotificationSender,
    ) -> Result<Self::Response> {
        protocol::dispatch(request, notification_tx, &self.state).await
    }
}
