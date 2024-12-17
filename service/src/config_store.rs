use serde::{de::DeserializeOwned, Serialize};
use std::{
    borrow::Borrow,
    fmt,
    io::{self, ErrorKind},
    marker::PhantomData,
    path::{Path, PathBuf},
    str,
};
use thiserror::Error;
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
};
use tracing::instrument;

pub(crate) struct ConfigStore {
    dir: PathBuf,
}

impl ConfigStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Obtain the config entry for the specified key.
    pub fn entry<T>(&self, key: ConfigKey<T>) -> ConfigEntry<'_, T> {
        ConfigEntry { store: self, key }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ConfigKey<T: 'static> {
    name: &'static str,
    comment: &'static str,
    _type: PhantomData<&'static T>,
}

impl<T> ConfigKey<T> {
    pub const fn new(name: &'static str, comment: &'static str) -> Self {
        Self {
            name,
            comment,
            _type: PhantomData,
        }
    }
}

pub(crate) struct ConfigEntry<'a, T>
where
    T: 'static,
{
    store: &'a ConfigStore,
    key: ConfigKey<T>,
}

impl<T> ConfigEntry<'_, T> {
    #[instrument(name = "config.set", skip_all, fields(key = self.key.name), err(Debug))]
    pub async fn set<U>(&self, value: &U) -> Result<(), ConfigError>
    where
        T: Borrow<U>,
        U: Serialize + fmt::Debug + ?Sized,
    {
        let path = self.path();

        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).await?;
        }

        // TODO: Consider doing this atomically by first writing to a .tmp file and then rename
        // once writing is done.
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .await?;

        for line in self.key.comment.lines() {
            file.write_all(format!("# {line}\n").as_bytes()).await?;
        }

        let content = serde_json::to_string_pretty(value)
            .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?;

        file.write_all(b"\n").await?;
        file.write_all(content.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;

        Ok(())
    }

    #[instrument(name = "config.remove", skip(self))]
    pub async fn remove(&self) -> Result<(), ConfigError> {
        match fs::remove_file(self.path()).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                tracing::error!(?error);
                Err(error.into())
            }
        }
    }

    pub(crate) fn path(&self) -> PathBuf {
        self.store.dir.join(self.key.name).with_extension("conf")
    }
}

impl<T> ConfigEntry<'_, T>
where
    T: DeserializeOwned + fmt::Debug,
{
    #[instrument(name = "config.get", skip(self), fields(key = self.key.name))]
    pub async fn get(&self) -> Result<T, ConfigError> {
        let path = self.path();

        let content = match fs::read(&path).await {
            Ok(content) => content,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ConfigError::NotFound);
            }
            Err(error) => {
                tracing::error!(?error);
                return Err(error.into());
            }
        };

        let content: String = str::from_utf8(&content)
            .map_err(|error| ConfigError::Malformed(Box::new(error)))
            .map_err(|error| {
                tracing::error!(?error);
                error
            })?
            .lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect();

        let value = serde_json::from_str(&content)
            .map_err(|error| ConfigError::Malformed(Box::new(error)))
            .map_err(|error| {
                tracing::error!(?error);
                error
            })?;

        Ok(value)
    }
}

impl<T> ConfigEntry<'_, T>
where
    T: Serialize + DeserializeOwned + fmt::Debug + Default,
{
    pub async fn modify<F>(&self, f: F) -> Result<(), ConfigError>
    where
        F: FnOnce(&mut T),
    {
        let mut value = match self.get().await {
            Ok(value) => value,
            Err(ConfigError::NotFound) => T::default(),
            Err(error) => return Err(error),
        };

        f(&mut value);

        self.set(&value).await
    }
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("config entry not found")]
    NotFound,
    #[error("config value is malformed")]
    Malformed(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("input/output error")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;
    use ouisync::PeerAddr;
    use tempfile::TempDir;

    #[tokio::test]
    async fn bool_entry() {
        let dir = TempDir::new().unwrap();
        let config = ConfigStore::new(dir.path());

        let key: ConfigKey<bool> = ConfigKey::new("bool", "first line\nsecond line\nthird line");
        let entry = config.entry(key);

        for value in [true, false] {
            entry.set(&value).await.unwrap();
            assert_eq!(entry.get().await.unwrap(), value);
        }
    }

    #[tokio::test]
    async fn u16_entry() {
        let dir = TempDir::new().unwrap();
        let config = ConfigStore::new(dir.path());

        let key: ConfigKey<u16> = ConfigKey::new("u16", "first line\nsecond line\nthird line");
        let entry = config.entry(key);

        for value in [0, 1, 2, 1000, u16::MAX] {
            entry.set(&value).await.unwrap();
            assert_eq!(entry.get().await.unwrap(), value);
        }
    }

    #[tokio::test]
    async fn string_entry() {
        let dir = TempDir::new().unwrap();
        let config = ConfigStore::new(dir.path());

        let key: ConfigKey<String> =
            ConfigKey::new("string", "first line\nsecond line\nthird line");
        let entry = config.entry(key);

        for value in [
            "foo",
            "bar",
            "baz qux",
            "first line\nsecond line\nthird line",
        ] {
            entry.set(value).await.unwrap();
            assert_eq!(entry.get().await.unwrap(), value);
        }
    }

    #[tokio::test]
    async fn peer_addr_entry() {
        let dir = TempDir::new().unwrap();
        let config = ConfigStore::new(dir.path());

        let key: ConfigKey<PeerAddr> =
            ConfigKey::new("peer_addr", "first line\nsecond line\nthird line");
        let entry = config.entry(key);

        for value in [
            PeerAddr::Quic((Ipv4Addr::LOCALHOST, 45000).into()),
            PeerAddr::Quic((Ipv6Addr::LOCALHOST, 45001).into()),
            PeerAddr::Tcp((Ipv6Addr::UNSPECIFIED, 45002).into()),
        ] {
            entry.set(&value).await.unwrap();
            assert_eq!(entry.get().await.unwrap(), value);
        }
    }

    #[tokio::test]
    async fn vec_of_peer_addr_entry() {
        let dir = TempDir::new().unwrap();
        let config = ConfigStore::new(dir.path());

        let key: ConfigKey<Vec<PeerAddr>> =
            ConfigKey::new("peer_addrs", "first line\nsecond line\nthird line");
        let entry = config.entry(key);

        for value in [
            vec![],
            vec![PeerAddr::Quic((Ipv4Addr::LOCALHOST, 45000).into())],
            vec![
                PeerAddr::Quic((Ipv6Addr::LOCALHOST, 45001).into()),
                PeerAddr::Quic((Ipv6Addr::LOCALHOST, 45002).into()),
            ],
        ] {
            entry.set(&value).await.unwrap();
            assert_eq!(entry.get().await.unwrap(), value);
        }
    }
}
