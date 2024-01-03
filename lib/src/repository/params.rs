use super::RepositoryMonitor;
use crate::{db, device_id::DeviceId, error::Result};
use state_monitor::{metrics::MetricsRecorder, StateMonitor};
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

pub struct RepositoryParams {
    store: Store,
    device_id: DeviceId,
    parent_monitor: Option<StateMonitor>,
    recorder: Option<MetricsRecorder>,
}

impl RepositoryParams {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self::with_store(Store::Path(path.as_ref().to_path_buf()))
    }

    #[cfg(test)]
    pub(crate) fn with_pool(pool: db::Pool, name: &str) -> Self {
        Self::with_store(Store::Pool {
            pool,
            name: name.to_owned(),
        })
    }

    fn with_store(store: Store) -> Self {
        Self {
            store,
            device_id: rand::random(),
            parent_monitor: None,
            recorder: None,
        }
    }

    pub fn with_device_id(self, device_id: DeviceId) -> Self {
        Self { device_id, ..self }
    }

    pub fn with_parent_monitor(self, parent_monitor: StateMonitor) -> Self {
        Self {
            parent_monitor: Some(parent_monitor),
            ..self
        }
    }

    pub fn with_recorder(self, recorder: MetricsRecorder) -> Self {
        Self {
            recorder: Some(recorder),
            ..self
        }
    }

    pub(super) async fn create(&self) -> Result<db::Pool, db::Error> {
        match &self.store {
            Store::Path(path) => db::create(path).await,
            #[cfg(test)]
            Store::Pool { pool, .. } => Ok(pool.clone()),
        }
    }

    pub(super) async fn open(&self) -> Result<db::Pool, db::Error> {
        match &self.store {
            Store::Path(path) => db::open(path).await,
            #[cfg(test)]
            Store::Pool { pool, .. } => Ok(pool.clone()),
        }
    }

    pub(super) fn device_id(&self) -> DeviceId {
        self.device_id
    }

    pub(super) fn monitor(&self) -> RepositoryMonitor {
        let name = self.store.name();

        let monitor = if let Some(parent_monitor) = &self.parent_monitor {
            parent_monitor.make_child(name)
        } else {
            StateMonitor::make_root()
        };

        let recorder = if let Some(recorder) = &self.recorder {
            Cow::Borrowed(recorder)
        } else {
            Cow::Owned(MetricsRecorder::new(monitor.clone()))
        };

        RepositoryMonitor::new(monitor, recorder.as_ref())
    }
}

enum Store {
    Path(PathBuf),
    #[cfg(test)]
    Pool {
        pool: db::Pool,
        name: String,
    },
}

impl Store {
    fn name(&self) -> Cow<'_, str> {
        match self {
            Self::Path(path) => path.as_os_str().to_string_lossy(),
            #[cfg(test)]
            Self::Pool { name, .. } => name.into(),
        }
    }
}
