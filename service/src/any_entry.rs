use crate::{error::Error, repository::RepositorySet};
use async_recursion::async_recursion;
use camino::{Utf8Path, Utf8PathBuf};
use ouisync::{EntryRef, EntryType, JointDirectory, JointEntryRef, Repository};
use std::{
    borrow::Cow,
    ffi::OsString,
    io,
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
            EntryType::File => AnyEntry::File(AnyFile::open_in_repo(repo, path).await?),
            EntryType::Directory => {
                AnyEntry::Dir(AnyDir::RepoRead(repo.open_directory(path).await?))
            }
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
    let entry = if let Some(repo) = repo {
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
            EntryType::File => AnyEntry::File(AnyFile::create_in_repo(repo, path).await?),
            EntryType::Directory => {
                let dir = repo.create_directory(path).await?;
                AnyEntry::Dir(AnyDir::RepoWrite(repo.clone(), dir, path.into()))
            }
        }
    } else {
        match fs::metadata(path).await {
            Ok(meta) => {
                if entry_type != EntryType::File || !meta.is_file() {
                    return Err(io::Error::from(io::ErrorKind::AlreadyExists).into());
                }
                AnyEntry::File(AnyFile::create_in_fs(path).await?)
            }
            Err(error) => {
                if error.kind() == io::ErrorKind::NotFound {
                    match entry_type {
                        EntryType::File => AnyEntry::File(AnyFile::create_in_fs(path).await?),
                        EntryType::Directory => {
                            fs::create_dir(path).await?;
                            AnyEntry::Dir(AnyDir::Fs(path.into()))
                        }
                    }
                } else {
                    return Err(error.into());
                }
            }
        }
    };
    Ok(entry)
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
    RepoWrite(Arc<Repository>, ouisync::Directory, Utf8PathBuf),
}

impl AnyDir {
    #[async_recursion]
    pub(crate) async fn copy_to(&self, dst: &mut AnyDir) -> Result<(), Error> {
        let mut entries = self.entries().await?;
        while let Some(entry) = entries.next().await? {
            match entry.open().await? {
                AnyEntry::File(mut src_file) => {
                    let mut dst_file = dst.create_file(&entry.name()).await?;
                    src_file.copy_to(&mut dst_file).await?;
                }
                AnyEntry::Dir(src_dir) => {
                    let mut dst_dir = dst.create_directory(&entry.name()).await?;
                    src_dir.copy_to(&mut dst_dir).await?;
                }
            }
        }
        Ok(())
    }

    async fn entries(&self) -> Result<AnyDirIter<'_>, Error> {
        let iter = match self {
            Self::Fs(path) => AnyDirIter::Fs(fs::read_dir(path).await?),
            Self::RepoRead(dir) => AnyDirIter::RepoRead(Box::new(dir.entries())),
            Self::RepoWrite(_repo, dir, _path) => AnyDirIter::RepoWrite(Box::new(dir.entries())),
        };
        Ok(iter)
    }

    async fn create_file(&mut self, name: &AnyFileName<'_>) -> Result<AnyFile, Error> {
        let file = match self {
            Self::Fs(parent) => AnyFile::create_in_fs(&parent.join(name.to_os_string())).await?,
            // We shouldn't reach this because the destination folder is always `RepoWrite`.
            // Still maybe returning an error would be more appropriate.
            Self::RepoRead(_dir) => unreachable!(),
            Self::RepoWrite(_repo, dir, _path) => {
                AnyFile::RepoFile(dir.create_file(name.to_utf8_string()?).await?)
            }
        };
        Ok(file)
    }

    async fn create_directory(&self, name: &AnyFileName<'_>) -> Result<AnyDir, Error> {
        let dir = match self {
            Self::Fs(parent_path) => {
                let path = parent_path.join(name.to_os_string());
                fs::create_dir(&path).await?;
                AnyDir::Fs(path)
            }
            // We shouldn't reach this because the destination folder is always `RepoWrite`.
            // Still maybe returning an error would be more appropriate.
            Self::RepoRead(_dir) => unreachable!(),
            Self::RepoWrite(repo, _parent, path) => {
                let path = path.join(name.to_utf8_string()?);
                let dir = repo.create_directory(&path).await?;
                AnyDir::RepoWrite(repo.clone(), dir, path)
            }
        };
        Ok(dir)
    }
}

enum AnyFileName<'a> {
    Fs(OsString),
    Repo(Cow<'a, str>),
}

impl<'a> AnyFileName<'a> {
    fn to_os_string(&self) -> OsString {
        match self {
            Self::Fs(s) => s.clone(),
            Self::Repo(s) => OsString::from(s.as_ref()),
        }
    }
    fn to_utf8_string(&self) -> Result<String, Error> {
        match self {
            Self::Fs(s) => s.to_str().ok_or(Error::InvalidArgument).map(|s| s.into()),
            Self::Repo(s) => Ok(s.to_string()),
        }
    }
}

enum EntryInfo<'a> {
    Fs(fs::DirEntry, std::fs::Metadata),
    RepoRead(JointEntryRef<'a>),
    RepoWrite(EntryRef<'a>),
}

impl<'a> EntryInfo<'a> {
    fn name(&self) -> AnyFileName<'a> {
        match self {
            Self::Fs(entry, _meta) => AnyFileName::Fs(entry.file_name()),
            Self::RepoRead(entry) => AnyFileName::Repo(entry.unique_name()),
            Self::RepoWrite(entry) => AnyFileName::Repo(Cow::Borrowed(entry.name())),
        }
    }

    async fn open(&self) -> Result<AnyEntry, Error> {
        let entry = match self {
            Self::Fs(entry, meta) => {
                if meta.is_file() {
                    AnyEntry::File(AnyFile::open_in_fs(&entry.path()).await?)
                } else if meta.is_dir() {
                    AnyEntry::Dir(AnyDir::Fs(entry.path()))
                } else {
                    return Err(ouisync::Error::OperationNotSupported.into());
                }
            }
            Self::RepoRead(entry) => match entry {
                ouisync::JointEntryRef::File(file_entry) => {
                    AnyEntry::File(AnyFile::RepoFile(file_entry.open().await?))
                }
                ouisync::JointEntryRef::Directory(dir_entry) => {
                    AnyEntry::Dir(AnyDir::RepoRead(dir_entry.open().await?))
                }
            },
            Self::RepoWrite(_entry) => todo!(),
        };
        Ok(entry)
    }
}

enum AnyDirIter<'a> {
    Fs(fs::ReadDir),
    RepoRead(Box<dyn Iterator<Item = JointEntryRef<'a>> + Send + 'a>),
    RepoWrite(Box<dyn Iterator<Item = EntryRef<'a>> + Send + 'a>),
}

impl<'a> AnyDirIter<'a> {
    async fn next(&mut self) -> Result<Option<EntryInfo<'a>>, Error> {
        let entry = match self {
            Self::Fs(iter) => match iter.next_entry().await? {
                Some(entry) => {
                    let meta = entry.metadata().await?;
                    Some(EntryInfo::Fs(entry, meta))
                }
                None => None,
            },
            Self::RepoRead(iter) => iter.next().map(EntryInfo::RepoRead),
            Self::RepoWrite(iter) => {
                for entry in iter.by_ref() {
                    match entry {
                        EntryRef::File(_) => {
                            return Ok(Some(EntryInfo::RepoWrite(entry)));
                        }
                        EntryRef::Directory(_) => {
                            return Ok(Some(EntryInfo::RepoWrite(entry)));
                        }
                        EntryRef::Tombstone(_) => continue,
                    }
                }
                None
            }
        };
        Ok(entry)
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
                Err(error.into())
            }
        }
    }

    pub(crate) async fn create_in_fs(path: &Path) -> Result<AnyFile, Error> {
        match fs::File::create(&path).await {
            Ok(file) => Ok(AnyFile::FsFile(file)),
            Err(error) => {
                tracing::error!("Cannot open file {path:?}: {:?}", error);
                Err(error.into())
            }
        }
    }

    pub(crate) async fn open_in_repo(repo: &Repository, path: &Utf8Path) -> Result<AnyFile, Error> {
        match repo.open_file(&path).await {
            Ok(file) => Ok(AnyFile::RepoFile(file)),
            Err(error) => {
                tracing::error!("Failed to create destination file: {:?}", error);
                Err(error.into())
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
                Err(error.into())
            }
        }
    }

    pub(crate) async fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Error> {
        match self {
            AnyFile::FsFile(file) => Ok(file.read(buffer).await?),
            AnyFile::RepoFile(file) => {
                return Ok(file.read(buffer).await?);
            }
        }
    }

    pub(crate) async fn write_all(&mut self, buffer: &[u8]) -> Result<(), Error> {
        match self {
            AnyFile::FsFile(file) => Ok(file.write_all(buffer).await?),
            AnyFile::RepoFile(file) => {
                return Ok(file.write_all(buffer).await?);
            }
        }
    }

    pub(crate) async fn flush(&mut self) -> Result<(), Error> {
        match self {
            AnyFile::FsFile(file) => Ok(file.flush().await?),
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
    let handle = match repos.find_by_subpath(name) {
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
            Err(Error::InvalidArgument)
        }
    }
}

fn to_utf8_path(path: &Path) -> Result<&camino::Utf8Path, Error> {
    match camino::Utf8Path::from_path(path) {
        Some(path) => Ok(path),
        None => {
            tracing::error!("Invalid repository path {path:?}");
            Err(Error::InvalidArgument)
        }
    }
}
