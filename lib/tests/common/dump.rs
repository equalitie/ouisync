//! In-memory Repository dumps.

use async_recursion::async_recursion;
use camino::Utf8Path;
use ouisync::{JointDirectory, JointEntryRef, Repository};
use std::{collections::BTreeMap, fmt, str};

#[derive(Eq, PartialEq, Debug)]
pub(crate) struct Directory(BTreeMap<String, Entry>);

impl Directory {
    pub const fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn add(mut self, name: impl Into<String>, entry: impl Into<Entry>) -> Self {
        self.0.insert(name.into(), entry.into());
        self
    }
}

#[derive(Eq, PartialEq)]
pub(crate) struct File(Vec<u8>);

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(s) = str::from_utf8(&self.0) {
            write!(f, "{s:?}")
        } else {
            write!(f, "{}", hex::encode(&self.0))
        }
    }
}

#[derive(Eq, PartialEq, Debug)]
pub(crate) enum Entry {
    File(File),
    Directory(Directory),
}

impl From<Vec<u8>> for Entry {
    fn from(content: Vec<u8>) -> Self {
        Self::File(File(content))
    }
}

impl From<Directory> for Entry {
    fn from(dir: Directory) -> Self {
        Self::Directory(dir)
    }
}

pub(crate) async fn save(repo: &Repository) -> Directory {
    save_directory(repo.open_directory("/").await.unwrap()).await
}

pub(crate) async fn load(repo: &Repository, dump: &Directory) {
    load_directory(repo, Utf8Path::new("/"), dump).await
}

#[async_recursion]
async fn save_directory(dir: JointDirectory) -> Directory {
    let mut entries = BTreeMap::new();

    for entry in dir.entries() {
        let name = entry.name().to_owned();
        let dump = match entry {
            JointEntryRef::File(entry) => {
                let mut file = entry.open().await.unwrap();
                Entry::File(File(file.read_to_end().await.unwrap()))
            }
            JointEntryRef::Directory(entry) => {
                let dir = entry.open().await.unwrap();
                Entry::Directory(save_directory(dir).await)
            }
        };

        entries.insert(name, dump);
    }

    Directory(entries)
}

#[async_recursion]
async fn load_directory(repo: &Repository, path: &Utf8Path, dump: &Directory) {
    for (name, dump) in &dump.0 {
        let path = path.join(name);

        match dump {
            Entry::File(dump) => {
                let mut file = repo.create_file(path).await.unwrap();
                file.write_all(&dump.0).await.unwrap();
                file.flush().await.unwrap();
            }
            Entry::Directory(dump) => {
                repo.create_directory(&path).await.unwrap();
                load_directory(repo, &path, dump).await;
            }
        }
    }
}
