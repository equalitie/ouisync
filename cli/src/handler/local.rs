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
            Request::Quota {
                name,
                default: _,
                remove,
                value,
            } => {
                let value = if remove { Some(None) } else { value.map(Some) };

                if let Some(name) = name {
                    let holder = self.state.repositories.find(&name)?;

                    if let Some(value) = value {
                        holder.repository.set_quota(value).await?;
                        Ok(().into())
                    } else {
                        let quota = holder.repository.quota().await?;
                        let size = holder.repository.size().await?;

                        Ok(QuotaInfo { quota, size }.into())
                    }
                } else if let Some(value) = value {
                    ouisync_bridge::repository::set_default_quota(&self.state.config, value)
                        .await?;
                    Ok(().into())
                } else if let Some(value) =
                    ouisync_bridge::repository::get_default_quota(&self.state.config).await?
                {
                    Ok(value.into())
                } else {
                    Ok(().into())
                }
            }
            Request::Expiration {
                name,
                default: _,
                remove,
                block_expiration,
                repository_expiration,
            } => {
                let [block_expiration, repository_expiration] =
                    [block_expiration, repository_expiration].map(|value| {
                        if remove {
                            Some(None)
                        } else {
                            value.map(Duration::from_secs).map(Some)
                        }
                    });

                if let Some(name) = name {
                    let holder = self.state.repositories.find(&name)?;

                    if let Some(value) = block_expiration {
                        holder.repository.set_block_expiration(value).await?;
                    }

                    if let Some(value) = repository_expiration {
                        holder.set_repository_expiration(value).await?;
                    }

                    Ok(Response::Expiration {
                        block: holder.repository.block_expiration(),
                        repository: holder.repository_expiration().await?,
                    })
                } else {
                    if let Some(value) = block_expiration {
                        ouisync_bridge::repository::set_default_block_expiration(
                            &self.state.config,
                            value,
                        )
                        .await?;
                    }

                    if let Some(value) = repository_expiration {
                        self.state.set_default_repository_expiration(value).await?;
                    }

                    Ok(Response::Expiration {
                        block: ouisync_bridge::repository::get_default_block_expiration(
                            &self.state.config,
                        )
                        .await?,
                        repository: self.state.default_repository_expiration().await?,
                    })
                }
            }
            Request::SetAccess { name, token } => {
                use std::str::FromStr;

                let holder = self.state.repositories.find(&name)?;
                let token = ShareToken::from_str(&token)?;
                let new_credentials = Credentials::with_random_writer_id(token.into_secrets());
                holder.repository.set_credentials(new_credentials).await?;
                Ok(().into())
            }
        }
    }
}
*/
