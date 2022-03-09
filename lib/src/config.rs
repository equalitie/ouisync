use std::{
    fmt,
    io::{self, ErrorKind},
    marker::PhantomData,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
};

#[derive(Clone)]
pub struct ConfigStore {
    dir: Option<Arc<Path>>,
}

impl ConfigStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Some(dir.into().into_boxed_path().into()),
        }
    }

    // Create "null" config store which doesn't actually store anything on the filesystem.
    pub fn null() -> Self {
        Self { dir: None }
    }

    // pub fn entry<'a, 'b, T>(&self, key: &'a ConfigKey<'b, T>) -> ConfigEntry<'a, 'b, T> {
    //     ConfigEntry {
    //         manager: self.clone(),
    //         key,
    //     }
    // }
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

// pub(crate) struct ConfigEntry<'a, 'b, T> {
//     manager: ConfigManager,
//     key: &'a ConfigKey<'b, T>,
// }

pub(crate) struct SingleValueConfig<Value>
where
    Value: fmt::Display + FromStr + 'static,
{
    path: Option<PathBuf>,
    key: ConfigKey<Value>,
}

impl<Value: fmt::Display + FromStr> SingleValueConfig<Value> {
    pub fn new(store: &ConfigStore, key: ConfigKey<Value>) -> Self {
        Self {
            path: store
                .dir
                .as_ref()
                .map(|dir| dir.join(key.name).with_extension("conf")),
            key,
        }
    }

    pub async fn set(&self, value: &Value) -> io::Result<()> {
        let path = if let Some(path) = &self.path {
            path
        } else {
            return Ok(());
        };

        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).await?;
        }

        // TODO: Consider doing this atomically by first writing to a .tmp file and then rename
        // once writing is done.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await?;

        file.write_all(format!("{}\n\n{}\n", self.key.comment, value).as_ref())
            .await
    }

    pub async fn get(&self) -> io::Result<Value> {
        let path = self
            .path
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "config value not found"))?;

        let file = File::open(path).await?;
        let line = self.find_value_line(file).await?;

        line.parse().map_err(|_| {
            io::Error::new(
                ErrorKind::InvalidData,
                format!("failed to convert data to value from file {:?}", self.path),
            )
        })
    }

    async fn find_value_line(&self, file: File) -> io::Result<String> {
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        while let Some(line) = lines.next_line().await? {
            let line = line.trim();

            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            return Ok(line.to_owned());
        }

        Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("no value found in {:?}", self.path),
        ))
    }
}
