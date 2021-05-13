use std::{
    ffi::OsStr,
    path::{Component, Path},
};

use crate::{
    crypto::Cryptor,
    db,
    directory::Directory,
    entry::{Entry, EntryType},
    error::{Error, Result},
    file::File,
    index::{Branch, Index},
    locator::Locator,
    this_replica,
};

pub struct Repository {
    pub index: Index,
    cryptor: Cryptor,
}

impl Repository {
    pub async fn new(pool: db::Pool, cryptor: Cryptor) -> Result<Self> {
        let this_replica_id = this_replica::get_or_create_id(&pool).await?;
        let index = Index::load(pool, this_replica_id).await?;

        Ok(Self { index, cryptor })
    }


    /// Opens the root directory.
    pub async fn root(&self) -> Result<Directory> {
        self.open_directory_by_locator(Locator::Root).await
    }

    /// Looks up an entry by its path. The path must be relative to the repository root.
    /// If the entry exists, returns its `Locator` and `EntryType`, otherwise returns
    /// `EntryNotFound`.
    pub async fn lookup<P: AsRef<Path>>(&self, path: P) -> Result<(Locator, EntryType)> {
        self.lookup_by_path(path.as_ref()).await
    }

    /// Opens a file at the given path (relative to the repository root)
    pub async fn open_file<P: AsRef<Path>>(&self, path: P) -> Result<File> {
        let (locator, entry_type) = self.lookup(path).await?;
        entry_type.check_is_file()?;
        self.open_file_by_locator(locator).await
    }

    /// Opens a directory at the given path (relative to the repository root)
    pub async fn open_directory<P: AsRef<Path>>(&self, path: P) -> Result<Directory> {
        let (locator, entry_type) = self.lookup(path).await?;
        entry_type.check_is_directory()?;
        self.open_directory_by_locator(locator).await
    }

    /// Removes (delete) the file at the given path. Returns the parent directory.
    pub async fn remove_file<P: AsRef<Path>>(&self, path: P) -> Result<Directory> {
        let (parent, name) = decompose_path(path.as_ref()).ok_or(Error::EntryIsDirectory)?;
        let mut parent = self.open_directory(parent).await?;
        parent.remove_file(name).await?;

        Ok(parent)
    }

    /// Removes the directory at the given path. The directory must be empty. Returns the parent
    /// directory.
    pub async fn remove_directory<P: AsRef<Path>>(&self, path: P) -> Result<Directory> {
        let (parent, name) = decompose_path(path.as_ref()).ok_or(Error::EntryIsDirectory)?;
        let mut parent = self.open_directory(parent).await?;
        parent.remove_subdirectory(name).await?;

        Ok(parent)
    }

    /// Open an entry (file or directory) at the given locator.
    pub async fn open_entry_by_locator(
        &self,
        locator: Locator,
        entry_type: EntryType,
    ) -> Result<Entry> {
        match entry_type {
            EntryType::File => Ok(Entry::File(self.open_file_by_locator(locator).await?)),
            EntryType::Directory => Ok(Entry::Directory(
                self.open_directory_by_locator(locator).await?,
            )),
        }
    }

    pub async fn open_file_by_locator(&self, locator: Locator) -> Result<File> {
        let branch = self.own_branch().await;

        File::open(
            self.index.pool.clone(),
            branch,
            self.cryptor.clone(),
            locator,
        )
        .await
    }

    pub async fn open_directory_by_locator(&self, locator: Locator) -> Result<Directory> {
        let branch = self.own_branch().await;

        match Directory::open(
            self.index.pool.clone(),
            branch.clone(),
            self.cryptor.clone(),
            locator,
        )
        .await
        {
            Ok(dir) => Ok(dir),
            Err(Error::EntryNotFound) if locator == Locator::Root => {
                // Lazily Create the root directory
                Ok(Directory::create(
                    self.index.pool.clone(),
                    branch,
                    self.cryptor.clone(),
                    Locator::Root,
                ))
            }
            Err(error) => Err(error),
        }
    }

    async fn own_branch(&self) -> Branch {
        self.index
            .branch(&self.index.this_replica_id)
            .await
            .unwrap()
    }

    async fn lookup_by_path(&self, path: &Path) -> Result<(Locator, EntryType)> {
        let mut stack = vec![Locator::Root];
        let mut last_type = EntryType::Directory;

        for component in path.components() {
            match component {
                Component::Prefix(_) => return Err(Error::OperationNotSupported),
                Component::RootDir | Component::CurDir => (),
                Component::ParentDir => {
                    if stack.len() > 1 {
                        stack.pop();
                    }

                    last_type = EntryType::Directory;
                }
                Component::Normal(name) => {
                    last_type.check_is_directory()?;

                    let parent_dir = self
                        .open_directory_by_locator(*stack.last().unwrap())
                        .await?;
                    let next_entry = parent_dir.lookup(name)?;

                    stack.push(next_entry.locator());
                    last_type = next_entry.entry_type();
                }
            }
        }

        Ok((stack.pop().unwrap(), last_type))
    }
}

/// Decomposes `Path` into parent and filename. Returns `None` if `path` doesn't have parent
/// (it's the root).
pub fn decompose_path(path: &Path) -> Option<(&Path, &OsStr)> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Some((parent, name)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn lookup() {
        let pool = db::init(db::Store::Memory).await.unwrap();
        let repo = Repository::new(pool, Cryptor::Null).await.unwrap();

        let mut root_dir = repo.open_directory_by_locator(Locator::Root).await.unwrap();
        let mut file_a = root_dir.create_file("a.txt".into()).unwrap();
        file_a.flush().await.unwrap();

        let mut subdir = root_dir.create_subdirectory("sub".into()).unwrap();
        let mut file_b = subdir.create_file("b.txt".into()).unwrap();
        file_b.flush().await.unwrap();
        subdir.flush().await.unwrap();

        root_dir.flush().await.unwrap();

        assert_eq!(
            repo.lookup("").await.unwrap(),
            (Locator::Root, EntryType::Directory)
        );
        assert_eq!(
            repo.lookup("sub").await.unwrap(),
            (*subdir.locator(), EntryType::Directory)
        );
        assert_eq!(
            repo.lookup("a.txt").await.unwrap(),
            (*file_a.locator(), EntryType::File)
        );
        assert_eq!(
            repo.lookup("sub/b.txt").await.unwrap(),
            (*file_b.locator(), EntryType::File)
        );

        assert_eq!(repo.lookup("/").await.unwrap().0, Locator::Root);
        assert_eq!(repo.lookup("//").await.unwrap().0, Locator::Root);
        assert_eq!(repo.lookup(".").await.unwrap().0, Locator::Root);
        assert_eq!(repo.lookup("/.").await.unwrap().0, Locator::Root);
        assert_eq!(repo.lookup("./").await.unwrap().0, Locator::Root);
        assert_eq!(repo.lookup("/sub").await.unwrap().0, *subdir.locator());
        assert_eq!(repo.lookup("sub/").await.unwrap().0, *subdir.locator());
        assert_eq!(repo.lookup("/sub/").await.unwrap().0, *subdir.locator());
        assert_eq!(repo.lookup("./sub").await.unwrap().0, *subdir.locator());
        assert_eq!(repo.lookup("././sub").await.unwrap().0, *subdir.locator());
        assert_eq!(repo.lookup("sub/.").await.unwrap().0, *subdir.locator());

        assert_eq!(repo.lookup("sub/..").await.unwrap().0, Locator::Root);
        assert_eq!(
            repo.lookup("sub/../a.txt").await.unwrap().0,
            *file_a.locator()
        );
        assert_eq!(repo.lookup("..").await.unwrap().0, Locator::Root);
        assert_eq!(repo.lookup("../..").await.unwrap().0, Locator::Root);
        assert_eq!(repo.lookup("sub/../..").await.unwrap().0, Locator::Root);
    }
}
