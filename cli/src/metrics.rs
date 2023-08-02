use crate::state::State;
use ouisync_bridge::{
    config::{ConfigError, ConfigKey},
    error::Error,
};
use scoped_task::ScopedAbortHandle;
use std::{net::SocketAddr, sync::Mutex};

const BIND_METRICS_KEY: ConfigKey<SocketAddr> =
    ConfigKey::new("bind_metrics", "Addresses to bind the metrics endpoint to");

pub(crate) struct MetricsServer {
    handle: Mutex<Option<ScopedAbortHandle>>,
}

impl MetricsServer {
    pub fn new() -> Self {
        Self {
            handle: Mutex::new(None),
        }
    }

    pub async fn init(&self, state: &State) -> Result<(), Error> {
        let entry = state.config.entry(BIND_METRICS_KEY);

        let addr = match entry.get().await {
            Ok(addr) => Some(addr),
            Err(ConfigError::NotFound) => None,
            Err(error) => return Err(error.into()),
        };

        if let Some(addr) = addr {
            let handle = start(state, addr).await?;
            *self.handle.lock().unwrap() = Some(handle);
        }

        Ok(())
    }

    pub async fn bind(&self, state: &State, addr: Option<SocketAddr>) -> Result<(), Error> {
        let entry = state.config.entry(BIND_METRICS_KEY);

        if let Some(addr) = addr {
            let handle = start(state, addr).await?;
            *self.handle.lock().unwrap() = Some(handle);
            entry.set(&addr).await?;
        } else {
            self.handle.lock().unwrap().take();
            entry.remove().await?;
        }

        Ok(())
    }

    pub fn close(&self) {
        self.handle.lock().unwrap().take();
    }
}

async fn start(_state: &State, _addr: SocketAddr) -> Result<ScopedAbortHandle, Error> {
    // let recorder = PrometheusBuilder::new().build_recorder();
    // let recorder_handle = recorder.handle();

    todo!()
}
