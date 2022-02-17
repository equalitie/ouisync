use std::{
    fmt,
    io::{self, ErrorKind},
    marker::PhantomData,
    path::{Path, PathBuf},
    str::FromStr,
};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
};

pub(crate) struct SingleValueConfig<Value>
where
    Value: fmt::Display + FromStr,
{
    path: PathBuf,
    comment: &'static str,
    phantom: PhantomData<Value>,
}

impl<Value: fmt::Display + FromStr> SingleValueConfig<Value> {
    pub fn new(path: &Path, comment: &'static str) -> Self {
        Self {
            path: path.to_path_buf(),
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
