use ouisync::DeviceId;
use rand::{rngs::OsRng, Rng};

use crate::{
    config_keys::DEVICE_ID_KEY,
    config_store::{ConfigError, ConfigStore},
};

pub(crate) async fn get_or_create(config: &ConfigStore) -> Result<DeviceId, ConfigError> {
    let entry = config.entry(DEVICE_ID_KEY);

    match entry.get().await {
        Ok(id) => Ok(id),
        Err(ConfigError::NotFound | ConfigError::Malformed(_)) => {
            let new_id = OsRng.gen();
            entry.set(&new_id).await?;
            Ok(new_id)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use tokio::fs;

    use super::*;

    #[tokio::test]
    async fn device_id_config_entry() {
        let dir = TempDir::new().unwrap();
        let config = ConfigStore::new(dir.path());

        let entry = config.entry(DEVICE_ID_KEY);
        let value = rand::random();

        entry.set(&value).await.unwrap();
        assert_eq!(entry.get().await.unwrap(), value);
    }

    #[tokio::test]
    async fn device_id_legacy_config_format() {
        let dir = TempDir::new().unwrap();
        let config = ConfigStore::new(dir.path());

        let device_id: DeviceId = rand::random();
        fs::write(
            config.entry(DEVICE_ID_KEY).path(),
            device_id.to_string().as_bytes(),
        )
        .await
        .unwrap();

        get_or_create(&config).await.unwrap();
    }
}
