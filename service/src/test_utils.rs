use futures_util::future::{AbortHandle, Abortable};
use tokio::task::{self, JoinHandle};
use tracing::{Instrument, Span};

use crate::Service;

pub(crate) fn init_log() {
    tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .with_test_writer()
        .try_init()
        .ok();
}

pub(crate) struct ServiceRunner {
    task: JoinHandle<Service>,
    abort_handle: AbortHandle,
}

impl ServiceRunner {
    pub fn start(mut service: Service) -> Self {
        // Using `AbortHandle` instead of aborting the task itself so we can get the service back
        // after abort.
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        let task = task::spawn(
            async move {
                match Abortable::new(service.run(), abort_registration).await {
                    Ok(Ok(())) => (),
                    Ok(Err(error)) => panic!("unexpected error: {error:?}"),
                    Err(_) => (),
                }

                service
            }
            .instrument(Span::current()),
        );

        Self { task, abort_handle }
    }

    pub async fn stop(self) -> Service {
        self.abort_handle.abort();
        self.task.await.unwrap()
    }
}
