use crate::{error::Error, repository::RepositorySet};
use camino::Utf8Path;
use ouisync::{EntryType, JointDirectory, Repository};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
};

/// Open file or directory. If `repo_name` is null, open it from the local file system. Otherwise
/// open it from the corresponding repository.
pub(crate) async fn open(repo: &Option<Arc<Repository>>, path: &Path) -> Result<AnyEntry, Error> {
    if let Some(repo) = repo {
        let path = to_utf8_path(path)?;
        let entry = match repo.lookup_type(&path).await? {
            EntryType::File => AnyEntry::File(AnyFile::open_in_repo(&repo, path).await?),
            EntryType::Directory => AnyEntry::Dir(AnyDir::open_in_repo(&repo, path).await?),
        };
        Ok(entry)
    } else {
        let metadata = fs::metadata(path).await?;
        let entry = if metadata.is_dir() {
            AnyEntry::Dir(AnyDir::Fs(path.into()))
        } else {
            AnyEntry::File(AnyFile::open_in_fs(path).await?)
        };
        Ok(entry)
    }
}

/// Create file or directory. If `repo_name` is null, create it on the local file system. Otherwise
/// create it in the corresponding repository.
pub(crate) async fn create(
    repo: &Option<Arc<Repository>>,
    path: &Path,
    entry_type: EntryType,
) -> Result<AnyEntry, Error> {
    let file = if let Some(repo) = repo {
        let path = to_utf8_path(path)?;
        match repo.lookup_type(&path).await {
            Ok(existing_entry_type) => {
                if existing_entry_type != entry_type {
                    return Err(Error::AlreadyExists);
                }
            }
            Err(ouisync::Error::EntryNotFound) => (),
            Err(error) => {
                return Err(error.into());
            }
        }
        match entry_type {
            EntryType::File => AnyFile::create_in_repo(&repo, path).await?,
            EntryType::Directory => todo!(),
        }
    } else {
        AnyFile::create_in_fs(path).await?
    };
    Ok(AnyEntry::File(file))
}

pub(crate) async fn is_directory(
    repo: &Option<Arc<Repository>>,
    path: &Path,
) -> Result<bool, Error> {
    let is_dir = match repo {
        Some(repo) => {
            let path = to_utf8_path(path)?;
            repo.lookup_type(path).await? == EntryType::Directory
        }
        None => fs::metadata(path).await?.is_dir(),
    };
    Ok(is_dir)
}

pub(crate) enum AnyEntry {
    File(AnyFile),
    Dir(AnyDir),
}

impl AnyEntry {
    pub(crate) fn entry_type(&self) -> EntryType {
        match self {
            Self::File(_) => EntryType::File,
            Self::Dir(_) => EntryType::Directory,
        }
    }
}

pub(crate) enum AnyDir {
    Fs(PathBuf),
    RepoRead(JointDirectory),
    RepoWrite(ouisync::Directory),
}

impl AnyDir {
    async fn open_in_repo(repo: &Repository, path: &Utf8Path) -> Result<AnyDir, Error> {
        Ok(AnyDir::RepoRead(repo.open_directory(path).await?))
    }
}

pub(crate) enum AnyFile {
    FsFile(fs::File),
    RepoFile(ouisync::File),
}

impl AnyFile {
    pub(crate) async fn open_in_fs(path: &Path) -> Result<AnyFile, Error> {
        match fs::File::open(path).await {
            Ok(file) => Ok(AnyFile::FsFile(file)),
            Err(error) => {
                tracing::error!("Cannot open file {path:?}: {:?}", error);
                return Err(error.into());
            }
        }
    }

    pub(crate) async fn create_in_fs(path: &Path) -> Result<AnyFile, Error> {
        match fs::File::create(&path).await {
            Ok(file) => Ok(AnyFile::FsFile(file)),
            Err(error) => {
                tracing::error!("Cannot open file {path:?}: {:?}", error);
                return Err(error.into());
            }
        }
    }

    pub(crate) async fn open_in_repo(repo: &Repository, path: &Utf8Path) -> Result<AnyFile, Error> {
        match repo.open_file(&path).await {
            Ok(file) => Ok(AnyFile::RepoFile(file)),
            Err(error) => {
                tracing::error!("Failed to create destination file: {:?}", error);
                return Err(error.into());
            }
        }
    }

    pub(crate) async fn create_in_repo(
        repo: &Repository,
        path: &Utf8Path,
    ) -> Result<AnyFile, Error> {
        match repo.create_file(&path).await {
            Ok(file) => Ok(AnyFile::RepoFile(file)),
            Err(error) => {
                tracing::error!("Failed to create destination file: {:?}", error);
                return Err(error.into());
            }
        }
    }

    pub(crate) async fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Error> {
        match self {
            AnyFile::FsFile(file) => {
                return Ok(file.read(buffer).await?);
            }
            AnyFile::RepoFile(file) => {
                return Ok(file.read(buffer).await?);
            }
        }
    }

    pub(crate) async fn write_all(&mut self, buffer: &[u8]) -> Result<(), Error> {
        match self {
            AnyFile::FsFile(file) => {
                return Ok(file.write_all(buffer).await?);
            }
            AnyFile::RepoFile(file) => {
                return Ok(file.write_all(buffer).await?);
            }
        }
    }

    pub(crate) async fn flush(&mut self) -> Result<(), Error> {
        match self {
            AnyFile::FsFile(file) => {
                return Ok(file.flush().await?);
            }
            AnyFile::RepoFile(file) => {
                return Ok(file.flush().await?);
            }
        }
    }

    pub(crate) async fn copy_to(&mut self, dst: &mut AnyFile) -> Result<(), Error> {
        let mut buffer = [0; 65536];

        loop {
            let n = self.read(&mut buffer).await?;

            if n == 0 {
                break;
            }

            dst.write_all(&buffer[0..n]).await?;
        }

        dst.flush().await
    }
}

pub(crate) fn find_repo_by_name(
    repos: &RepositorySet,
    name: &str,
) -> Result<Arc<Repository>, Error> {
    let handle = match repos.find_by_subpath(&name) {
        Ok(handle) => handle,
        Err(error) => {
            tracing::error!("Cannot open repository {name:?}: {error:?}");
            return Err(error.into());
        }
    };
    match repos.get_repository(handle) {
        Some(repo) => Ok(repo),
        None => {
            tracing::error!("Failed to convert repo handle for: {name:?}");
            return Err(Error::InvalidArgument);
        }
    }
}

fn to_utf8_path(path: &Path) -> Result<&camino::Utf8Path, Error> {
    match camino::Utf8Path::from_path(path) {
        Some(path) => Ok(path),
        None => {
            tracing::error!("Invalid repository path {path:?}");
            return Err(Error::InvalidArgument);
        }
    }
}
