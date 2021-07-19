use crate::{
    crypto::Cryptor,
    db,
    directory::{Directory, MoveDstDirectory},
    entry::{Entry, EntryType},
    error::{Error, Result},
    file::File,
    index::BranchData,
    locator::Locator,
};

use camino::{Utf8Component, Utf8Path};

pub struct Branch {
    pool: db::Pool,
    branch_data: BranchData,
    cryptor: Cryptor,
}

impl Branch {
    pub fn new(pool: db::Pool, branch_data: BranchData, cryptor: Cryptor) -> Self {
        Self { pool, branch_data, cryptor }
    }

    pub fn data(&self) -> &BranchData {
        &self.branch_data
    }

    /// Looks up an entry by its path. The path must be relative to the repository root.
    /// If the entry exists, returns its `Locator` and `EntryType`, otherwise returns
    /// `EntryNotFound`.
    pub async fn lookup<P: AsRef<Utf8Path>>(&self, path: P) -> Result<(Locator, EntryType)> {
        self.lookup_by_path(path.as_ref()).await
    }

    /// Opens a file at the given path (relative to the repository root)
    pub async fn open_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<File> {
        let (locator, entry_type) = self.lookup(path).await?;
        entry_type.check_is_file()?;
        self.open_file_by_locator(locator).await
    }

    /// Opens a directory at the given path (relative to the repository root)
    pub async fn open_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<Directory> {
        let (locator, entry_type) = self.lookup(path).await?;
        entry_type.check_is_directory()?;
        self.open_directory_by_locator(locator).await
    }

    /// Creates a new file at the given path. Returns both the new file and its parent directory.
    pub async fn create_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<(File, Directory)> {
        let (parent, name) = decompose_path(path.as_ref()).ok_or(Error::EntryExists)?;

        let mut parent = self.open_directory(parent).await?;
        let file = parent.create_file(name.to_owned())?;

        Ok((file, parent))
    }

    /// Creates a new directory at the given path. Returns both the new directory and its parent
    /// directory.
    pub async fn create_directory<P: AsRef<Utf8Path>>(
        &self,
        path: P,
    ) -> Result<(Directory, Directory)> {
        let (parent, name) = decompose_path(path.as_ref()).ok_or(Error::EntryExists)?;

        let mut parent = self.open_directory(parent).await?;
        let dir = parent.create_directory(name.to_owned())?;

        Ok((dir, parent))
    }

    /// Removes (delete) the file at the given path. Returns the parent directory.
    pub async fn remove_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<Directory> {
        let (parent, name) = decompose_path(path.as_ref()).ok_or(Error::EntryIsDirectory)?;
        let mut parent = self.open_directory(parent).await?;
        parent.remove_file(name).await?;

        Ok(parent)
    }

    /// Removes the directory at the given path. The directory must be empty. Returns the parent
    /// directory.
    pub async fn remove_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<Directory> {
        let (parent, name) = decompose_path(path.as_ref()).ok_or(Error::EntryIsDirectory)?;
        let mut parent = self.open_directory(parent).await?;
        parent.remove_directory(name).await?;

        Ok(parent)
    }

    /// Moves (renames) an entry from the source path to the destination path.
    /// If both source and destination refer to the same entry, this is a no-op.
    /// Returns the parent directories of both `src` and `dst`.
    pub async fn move_entry<S: AsRef<Utf8Path>, D: AsRef<Utf8Path>>(
        &self,
        src: S,
        dst: D,
    ) -> Result<(Directory, MoveDstDirectory)> {
        // `None` here means we are trying to move the root which is not supported.
        let (src_parent, src_name) =
            decompose_path(src.as_ref()).ok_or(Error::OperationNotSupported)?;
        // `None` here means we are trying to move over the root which is a special case of moving
        // over existing directory which is not allowed.
        let (dst_parent, dst_name) = decompose_path(dst.as_ref()).ok_or(Error::EntryIsDirectory)?;

        // TODO: check that dst is not in a subdirectory of src

        let mut src_parent = self.open_directory(src_parent).await?;

        let (dst_parent_locator, dst_parent_type) = self.lookup(dst_parent).await?;
        dst_parent_type.check_is_directory()?;

        let mut dst_parent = if &dst_parent_locator == src_parent.locator() {
            MoveDstDirectory::Src
        } else {
            MoveDstDirectory::Other(self.open_directory_by_locator(dst_parent_locator).await?)
        };

        src_parent
            .move_entry(src_name, &mut dst_parent, dst_name)
            .await?;

        Ok((src_parent, dst_parent))
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
        File::open(
            self.pool.clone(),
            self.branch_data.clone(),
            self.cryptor.clone(),
            locator,
        )
        .await
    }

    pub async fn open_directory_by_locator(&self, locator: Locator) -> Result<Directory> {
        Directory::open(
            self.pool.clone(),
            self.branch_data.clone(),
            self.cryptor.clone(),
            locator,
        )
        .await
    }

    pub async fn ensure_root_exists(&self) -> Result<Directory> {
        match self.open_directory_by_locator(Locator::Root).await {
            Ok(dir) => Ok(dir),
            Err(Error::EntryNotFound) => {
                Ok(Directory::create(
                    self.pool.clone(),
                    self.branch_data.clone(),
                    self.cryptor.clone(),
                    Locator::Root,
                ))
            }
            Err(error) => Err(error),
        }
    }

    async fn lookup_by_path(&self, path: &Utf8Path) -> Result<(Locator, EntryType)> {
        let mut stack = vec![Locator::Root];
        let mut last_type = EntryType::Directory;

        for component in path.components() {
            match component {
                Utf8Component::Prefix(_) => return Err(Error::OperationNotSupported),
                Utf8Component::RootDir | Utf8Component::CurDir => (),
                Utf8Component::ParentDir => {
                    if stack.len() > 1 {
                        stack.pop();
                    }

                    last_type = EntryType::Directory;
                }
                Utf8Component::Normal(name) => {
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

// Decomposes `Path` into parent and filename. Returns `None` if `path` doesn't have parent
// (it's the root).
fn decompose_path(path: &Utf8Path) -> Option<(&Utf8Path, &str)> {
    match (path.parent(), path.file_name()) {
        // It's OK to use unwrap here because all file names are assumed to be UTF-8 (checks are
        // made in VirtualFilesystem).
        (Some(parent), Some(name)) => Some((parent, name)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, replica_id::ReplicaId, index::Index};

    #[tokio::test(flavor = "multi_thread")]
    async fn lookup() {
        let pool = db::init(db::Store::Memory).await.unwrap();
        let branch_id = ReplicaId::random();
        let index = Index::load(pool.clone(), branch_id).await.unwrap();
        let branch = Branch::new(pool, index.branch(&branch_id).await.unwrap(), Cryptor::Null);

        let mut root_dir = branch.ensure_root_exists().await.unwrap();
        let mut file_a = root_dir.create_file("a.txt".into()).unwrap();
        file_a.flush().await.unwrap();

        let mut subdir = root_dir.create_directory("sub".into()).unwrap();
        let mut file_b = subdir.create_file("b.txt".into()).unwrap();
        file_b.flush().await.unwrap();
        subdir.flush().await.unwrap();

        root_dir.flush().await.unwrap();

        assert_eq!(
            branch.lookup("").await.unwrap(),
            (Locator::Root, EntryType::Directory)
        );
        assert_eq!(
            branch.lookup("sub").await.unwrap(),
            (*subdir.locator(), EntryType::Directory)
        );
        assert_eq!(
            branch.lookup("a.txt").await.unwrap(),
            (*file_a.locator(), EntryType::File)
        );
        assert_eq!(
            branch.lookup("sub/b.txt").await.unwrap(),
            (*file_b.locator(), EntryType::File)
        );

        assert_eq!(branch.lookup("/").await.unwrap().0, Locator::Root);
        assert_eq!(branch.lookup("//").await.unwrap().0, Locator::Root);
        assert_eq!(branch.lookup(".").await.unwrap().0, Locator::Root);
        assert_eq!(branch.lookup("/.").await.unwrap().0, Locator::Root);
        assert_eq!(branch.lookup("./").await.unwrap().0, Locator::Root);
        assert_eq!(
            branch.lookup("/sub").await.unwrap().0,
            *subdir.locator()
        );
        assert_eq!(
            branch.lookup("sub/").await.unwrap().0,
            *subdir.locator()
        );
        assert_eq!(
            branch.lookup("/sub/").await.unwrap().0,
            *subdir.locator()
        );
        assert_eq!(
            branch.lookup("./sub").await.unwrap().0,
            *subdir.locator()
        );
        assert_eq!(
            branch.lookup("././sub").await.unwrap().0,
            *subdir.locator()
        );
        assert_eq!(
            branch.lookup("sub/.").await.unwrap().0,
            *subdir.locator()
        );

        assert_eq!(
            branch.lookup("sub/..").await.unwrap().0,
            Locator::Root
        );
        assert_eq!(
            branch.lookup("sub/../a.txt").await.unwrap().0,
            *file_a.locator()
        );
        assert_eq!(branch.lookup("..").await.unwrap().0, Locator::Root);
        assert_eq!(branch.lookup("../..").await.unwrap().0, Locator::Root);
        assert_eq!(
            branch.lookup("sub/../..").await.unwrap().0,
            Locator::Root
        );
    }
}
