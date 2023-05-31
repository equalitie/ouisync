use super::RepositoryMonitor;
use crate::{db, device_id::DeviceId, error::Result, state_monitor::StateMonitor, timing::Clocks};
use std::path::{Path, PathBuf};

pub struct RepositoryParams {
    store: Store,
    device_id: DeviceId,
    parent_monitor: StateMonitor,
    clocks: Clocks,
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
            parent_monitor: StateMonitor::make_root(),
            clocks: Clocks::new(),
        }
    }

    pub fn with_device_id(self, device_id: DeviceId) -> Self {
        Self { device_id, ..self }
    }

    pub fn with_parent_monitor(self, parent_monitor: StateMonitor) -> Self {
        Self {
            parent_monitor,
            ..self
        }
    }

    pub fn with_clocks(self, clocks: Clocks) -> Self {
        Self { clocks, ..self }
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
        let name = match &self.store {
            Store::Path(path) => path.as_os_str().to_string_lossy(),
            #[cfg(test)]
            Store::Pool { name, .. } => name.into(),
        };

        RepositoryMonitor::new(
            self.parent_monitor.clone(),
            self.clocks.clone(),
            name.as_ref(),
        )
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
