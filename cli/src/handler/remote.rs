use crate::{
    repository::{RepositoryHolder, RepositoryName, OPEN_ON_START},
    state::State,
};
use async_trait::async_trait;
use ouisync_bridge::{
    error::{Error, Result},
    protocol::remote::{Request, Response},
    transport::NotificationSender,
};
use ouisync_lib::{AccessMode, ShareToken};
use std::sync::{Arc, Weak};

#[derive(Clone)]
pub(crate) struct RemoteHandler {
    state: Weak<State>,
}

impl RemoteHandler {
    pub fn new(state: Arc<State>) -> Self {
        Self {
            state: Arc::downgrade(&state),
        }
    }
}

#[async_trait]
impl ouisync_bridge::transport::Handler for RemoteHandler {
    type Request = Request;
    type Response = Response;

    async fn handle(
        &self,
        request: Self::Request,
        _notification_tx: &NotificationSender,
    ) -> Result<Self::Response> {
        tracing::debug!(?request);

        let Some(state) = self.state.upgrade() else {
            tracing::error!("can't handle request - shutting down");
            // TODO: return more appropriate error (ShuttingDown or similar)
            return Err(Error::ForbiddenRequest);
        };

        match request {
            Request::Create { share_token } => {
                let share_token: ShareToken =
                    share_token.parse().map_err(|_| Error::InvalidArgument)?;
                // We support remote creation of blind replicas only.
                let share_token: ShareToken = share_token
                    .into_secrets()
                    .with_mode(AccessMode::Blind)
                    .into();

                let name = share_token
                    .id()
                    .salted_hash(b"ouisync server repository name")
                    .to_string();
                // unwrap ok because the name is just a string of hexadecimal digits which is
                // always a valid name.
                let name = RepositoryName::try_from(name).unwrap();

                let store_path = state.store_path(name.as_ref());

                let repository = ouisync_bridge::repository::create(
                    store_path.clone(),
                    None,
                    None,
                    Some(share_token),
                    &state.config,
                    &state.repositories_monitor,
                )
                .await?;

                repository.metadata().set(OPEN_ON_START, true).await.ok();

                tracing::info!(%name, "repository created");

                let holder = RepositoryHolder::new(repository, name, &state.network).await;
                let holder = Arc::new(holder);
                state.repositories.insert(holder);

                // TODO: enable DHT, PEX

                Ok(().into())
            }
        }
    }
}
