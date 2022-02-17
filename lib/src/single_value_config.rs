use std::{
    fmt,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    str::FromStr,
};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
};

pub(crate) struct SingleValueConfig {
    path: PathBuf,
    comment: &'static str,
}

impl SingleValueConfig {
    pub fn new(path: &Path, comment: &'static str) -> Self {
        Self {
            path: path.to_path_buf(),
            comment,
        }
    }

    pub async fn set<Value: fmt::Display>(&self, value: &Value) -> io::Result<()> {
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

    pub async fn get<Value: FromStr>(&self) -> io::Result<Value> {
        let file = File::open(&self.path).await?;
        let string = self.parse(file).await?;

        string.parse().map_err(|_| {
            io::Error::new(
                ErrorKind::InvalidData,
                format!("failed to convert data to value from file {:?}", self.path),
            )
        })
    }

    async fn parse(&self, file: File) -> io::Result<String> {
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
