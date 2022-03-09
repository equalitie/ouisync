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
    dir: Arc<Path>,
}

impl ConfigStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into().into_boxed_path().into(),
        }
    }

    pub fn temp() -> Self {
        Self { dir: todo!() }
    }

    // pub fn entry<'a, 'b, T>(&self, key: &'a ConfigKey<'b, T>) -> ConfigEntry<'a, 'b, T> {
    //     ConfigEntry {
    //         manager: self.clone(),
    //         key,
    //     }
    // }
}

// pub(crate) struct ConfigKey<'b, T> {
//     name: &'b str,
//     comment: &'b str,
//     _type: PhantomData<T>,
// }

// impl<'b, T> ConfigKey<'b, T> {
//     pub const fn new(name: &'b str, comment: &'b str) -> Self {
//         Self {
//             name,
//             comment,
//             _type: PhantomData,
//         }
//     }
// }

// pub(crate) struct ConfigEntry<'a, 'b, T> {
//     manager: ConfigManager,
//     key: &'a ConfigKey<'b, T>,
// }

pub(crate) struct SingleValueConfig<Value>
where
    Value: fmt::Display + FromStr,
{
    path: PathBuf,
    comment: &'static str,
    phantom: PhantomData<Value>,
}

impl<Value: fmt::Display + FromStr> SingleValueConfig<Value> {
    pub fn new(store: &ConfigStore, key: &str, comment: &'static str) -> Self {
        Self {
            path: store.dir.join(key),
            comment,
            phantom: PhantomData,
        }
    }

    pub async fn set(&self, value: &Value) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).await?;
        }

        // TODO: Consider doing this atomically by first writing to a .tmp file and then rename
        // once writing is done.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)
            .await?;

        file.write_all(format!("{}\n\n{}\n", self.comment, value).as_ref())
            .await
    }

    pub async fn get(&self) -> io::Result<Value> {
        let file = File::open(&self.path).await?;
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
