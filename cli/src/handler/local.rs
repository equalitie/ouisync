/*
use crate::{
    protocol::{ImportMode, ProtocolError, QuotaInfo, Request, Response},
    repository::{RepositoryName, RepositoryNameInvalid},
    state::{CreateRepositoryMethod, State},
    DB_EXTENSION,
};
use async_trait::async_trait;
use ouisync_bridge::{network, transport::SessionContext};
use ouisync_lib::{crypto::Password, Credentials, LocalSecret, ShareToken};
use std::{sync::Arc, time::Duration};
use tokio::fs;

#[derive(Clone)]
pub(crate) struct LocalHandler {
    state: Arc<State>,
}

impl LocalHandler {
    pub fn new(state: Arc<State>) -> Self {
        Self { state }
    }

    pub async fn close(&self) {
        self.state.close().await
    }
}

#[async_trait]
impl ouisync_bridge::transport::Handler for LocalHandler {
    type Request = Request;
    type Response = Response;
    type Error = ProtocolError;

    async fn handle(
        &self,
        request: Self::Request,
        _context: &SessionContext,
    ) -> Result<Self::Response, Self::Error> {
        match request {
            Request::BindRpc { addrs } => Ok(self
                .state
                .rpc_servers
                .set(self.state.clone(), &addrs)
                .await?
                .into()),
            Request::Mirror { name, host } => {
                let holder = self.state.repositories.find(&name)?;
                let config = self.state.get_client_config().await?;

                holder.mirror(&host, config).await?;

                Ok(().into())
            }
        }
    }
}
*/
