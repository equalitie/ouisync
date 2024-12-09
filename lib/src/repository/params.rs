use super::monitor::RepositoryMonitor;
use crate::{db, device_id::DeviceId, error::Result};
use metrics::{NoopRecorder, Recorder};
use state_monitor::{metrics::MetricsRecorder, StateMonitor};
use std::path::{Path, PathBuf};

pub struct RepositoryParams<R> {
    store: Store,
    device_id: DeviceId,
    monitor: Option<StateMonitor>,
    recorder: Option<R>,
}

impl<R> RepositoryParams<R> {
    pub fn with_device_id(self, device_id: DeviceId) -> Self {
        Self { device_id, ..self }
    }

    pub fn with_monitor(self, monitor: StateMonitor) -> Self {
        Self {
            monitor: Some(monitor),
            ..self
        }
    }

    pub fn with_recorder<S>(self, recorder: S) -> RepositoryParams<S> {
        RepositoryParams {
            store: self.store,
            device_id: self.device_id,
            monitor: self.monitor,
            recorder: Some(recorder),
        }
    }

    pub(super) async fn create(&self) -> Result<db::Pool, db::Error> {
        match &self.store {
            Store::Path(path) => db::create(path).await,
            #[cfg(test)]
            Store::Pool(pool) => Ok(pool.clone()),
        }
    }

    pub(super) async fn open(&self) -> Result<db::Pool, db::Error> {
        match &self.store {
            Store::Path(path) => db::open(path).await,
            #[cfg(test)]
            Store::Pool(pool) => Ok(pool.clone()),
        }
    }

    pub(super) fn device_id(&self) -> DeviceId {
        self.device_id
    }
}

impl<R> RepositoryParams<R>
where
    R: Recorder,
{
    pub(super) fn monitor(&self) -> RepositoryMonitor {
        let monitor = if let Some(monitor) = &self.monitor {
            monitor.clone()
        } else {
            StateMonitor::make_root()
        };

        if let Some(recorder) = &self.recorder {
            RepositoryMonitor::new(monitor, recorder)
        } else {
            RepositoryMonitor::new(monitor.clone(), &MetricsRecorder::new(monitor))
        }
    }
}

impl RepositoryParams<NoopRecorder> {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self::with_store(Store::Path(path.as_ref().to_path_buf()))
    }

    #[cfg(test)]
    pub(crate) fn with_pool(pool: db::Pool) -> Self {
        Self::with_store(Store::Pool(pool))
    }

    fn with_store(store: Store) -> Self {
        Self {
            store,
            device_id: rand::random(),
            monitor: None,
            recorder: None,
        }
    }
}

enum Store {
    Path(PathBuf),
    #[cfg(test)]
    Pool(db::Pool),
}
