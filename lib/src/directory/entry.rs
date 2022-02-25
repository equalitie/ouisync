use super::{
    entry_data::{EntryData, EntryDirectoryData, EntryFileData, EntryTombstoneData},
    inner::Inner,
    parent_context::ParentContext,
    Directory,
};
use crate::{
    blob_id::BlobId,
    crypto::sign::PublicKey,
    error::{Error, Result},
    file::File,
    locator::Locator,
    version_vector::VersionVector,
};
use std::{fmt, future::Future, sync::Arc};

/// Info about a directory entry.
#[derive(Copy, Clone, Debug)]
pub enum EntryRef<'a> {
    File(FileRef<'a>),
    Directory(DirectoryRef<'a>),
    Tombstone(TombstoneRef<'a>),
}

impl<'a> EntryRef<'a> {
    pub(super) fn new(
        parent_outer: &'a Directory,
        parent_inner: &'a Inner,
        name: &'a str,
        entry_data: &'a EntryData,
        author: &'a PublicKey,
    ) -> Self {
        let inner = RefInner {
            parent_outer,
            parent_inner,
            name,
            author,
        };

        match entry_data {
            EntryData::File(entry_data) => Self::File(FileRef { entry_data, inner }),
            EntryData::Directory(entry_data) => Self::Directory(DirectoryRef { entry_data, inner }),
            EntryData::Tombstone(entry_data) => Self::Tombstone(TombstoneRef { entry_data, inner }),
        }
    }

    pub fn name(&self) -> &'a str {
        match self {
            Self::File(r) => r.name(),
            Self::Directory(r) => r.name(),
            Self::Tombstone(r) => r.name(),
        }
    }

    pub fn author(&self) -> &PublicKey {
        self.inner().author
    }

    pub fn version_vector(&self) -> &VersionVector {
        match self {
            Self::File(f) => f.version_vector(),
            Self::Directory(d) => d.version_vector(),
            Self::Tombstone(t) => t.version_vector(),
        }
    }

    pub fn file(self) -> Result<FileRef<'a>> {
        match self {
            Self::File(r) => Ok(r),
            Self::Directory(_) => Err(Error::EntryIsDirectory),
            Self::Tombstone(_) => Err(Error::EntryNotFound),
        }
    }

    pub fn directory(self) -> Result<DirectoryRef<'a>> {
        match self {
            Self::File(_) => Err(Error::EntryIsFile),
            Self::Directory(r) => Ok(r),
            Self::Tombstone(_) => Err(Error::EntryNotFound),
        }
    }

    pub fn is_file(&self) -> bool {
        matches!(self, Self::File(_))
    }

    pub fn is_directory(&self) -> bool {
        matches!(self, Self::Directory(_))
    }

    pub fn is_tombstone(&self) -> bool {
        matches!(self, Self::Tombstone(_))
    }

    pub fn branch_id(&self) -> &PublicKey {
        self.inner().branch_id()
    }

    pub fn parent(&self) -> &Directory {
        self.inner().parent_outer
    }

    pub(crate) fn clone_data(&self) -> EntryData {
        match self {
            Self::File(e) => EntryData::File(e.data().clone()),
            Self::Directory(e) => EntryData::Directory(e.data().clone()),
            Self::Tombstone(e) => EntryData::Tombstone(e.data().clone()),
        }
    }

    fn inner(&self) -> &RefInner {
        match self {
            Self::File(r) => &r.inner,
            Self::Directory(r) => &r.inner,
            Self::Tombstone(r) => &r.inner,
        }
    }
}

#[derive(Copy, Clone)]
pub struct FileRef<'a> {
    entry_data: &'a EntryFileData,
    inner: RefInner<'a>,
}

impl<'a> FileRef<'a> {
    pub fn name(&self) -> &'a str {
        self.inner.name
    }

    pub(crate) fn locator(&self) -> Locator {
        Locator::head(*self.blob_id())
    }

    pub(crate) fn blob_id(&self) -> &BlobId {
        &self.entry_data.blob_id
    }

    pub fn author(&self) -> &'a PublicKey {
        self.inner.author
    }

    pub fn version_vector(&self) -> &'a VersionVector {
        &self.entry_data.version_vector
    }

    /// This function deliberately returns a future that does NOT borrow from `self`. This is why
    /// it's not a regular `async` function but rather returns explicit `impl Future`. The reason
    /// for this is to prevent deadlocks when opening a file while its parent directory is locked.
    pub fn open(&self) -> impl Future<Output = Result<File>> {
        let parent_context = self.inner.parent_context();
        let branch = self.inner.parent_inner.blob.branch().clone();
        let locator = self.locator();

        // TODO: This is a quick fix to a file corruption problem caused by one thread writing to a
        // file and another thread reading and closing it periodically. The problem was that both
        // files shared the same blob::Core and when the blob for reading was being closed, the
        // Blob::flush function was called which detected that the size has changed and overwrote
        // the first block with outdated data.
        File::open(branch, locator, parent_context)

        //let core = self.entry_data.blob_core.clone();
        //async move {
        //    let mut core = core.lock().await;

        //    if let Some(core) = core.upgrade() {
        //        File::reopen(branch, locator, parent_context, core).await
        //    } else {
        //        let file = File::open(branch, locator, parent_context).await?;
        //        *core = Arc::downgrade(file.blob_core());

        //        Ok(file)
        //    }
        //}
    }

    pub fn branch_id(&self) -> &PublicKey {
        self.inner.branch_id()
    }

    pub fn parent(&self) -> &Directory {
        self.inner.parent_outer
    }

    pub(crate) fn data(&self) -> &EntryFileData {
        self.entry_data
    }
}

impl fmt::Debug for FileRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("FileRef")
            .field("name", &self.inner.name)
            .field("author", &self.inner.author)
            .field("vv", &self.entry_data.version_vector)
            .field("locator", &self.locator())
            .finish()
    }
}

#[derive(Copy, Clone)]
pub struct DirectoryRef<'a> {
    entry_data: &'a EntryDirectoryData,
    inner: RefInner<'a>,
}

impl<'a> DirectoryRef<'a> {
    pub fn name(&self) -> &'a str {
        self.inner.name
    }

    pub(crate) fn locator(&self) -> Locator {
        Locator::head(self.entry_data.blob_id)
    }

    pub fn author(&self) -> &'a PublicKey {
        self.inner.author
    }

    pub async fn open(&self) -> Result<Directory> {
        self.inner
            .parent_inner
            .open_directories
            .open(
                self.inner.parent_inner.blob.branch().clone(),
                self.locator(),
                self.inner.parent_context(),
            )
            .await
    }

    pub fn branch_id(&self) -> &'a PublicKey {
        self.inner.branch_id()
    }

    pub(crate) fn data(&self) -> &EntryDirectoryData {
        self.entry_data
    }

    pub fn version_vector(&self) -> &VersionVector {
        &self.entry_data.version_vector
    }
}

impl fmt::Debug for DirectoryRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("DirectoryRef")
            .field("name", &self.inner.name)
            .finish()
    }
}

#[derive(Copy, Clone)]
pub struct TombstoneRef<'a> {
    entry_data: &'a EntryTombstoneData,
    inner: RefInner<'a>,
}

impl<'a> TombstoneRef<'a> {
    pub fn name(&self) -> &'a str {
        self.inner.name
    }

    pub fn branch_id(&self) -> &'a PublicKey {
        self.inner.branch_id()
    }

    pub fn data(&self) -> &EntryTombstoneData {
        self.entry_data
    }

    pub fn author(&self) -> &'a PublicKey {
        self.inner.author
    }

    pub fn version_vector(&self) -> &VersionVector {
        &self.entry_data.version_vector
    }
}

impl fmt::Debug for TombstoneRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("TombstoneRef")
            .field("vv", &self.entry_data.version_vector)
            .finish()
    }
}

#[derive(Copy, Clone)]
struct RefInner<'a> {
    parent_outer: &'a Directory,
    parent_inner: &'a Inner,
    name: &'a str,
    author: &'a PublicKey,
}

impl<'a> RefInner<'a> {
    fn parent_context(&self) -> ParentContext {
        ParentContext::new(
            self.parent_outer.branch_id,
            self.parent_outer.inner.clone(),
            self.name.into(),
            *self.author,
        )
    }

    pub fn branch_id(&self) -> &'a PublicKey {
        self.parent_inner.blob.branch().id()
    }
}

impl PartialEq for RefInner<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.parent_inner.blob.branch().id() == other.parent_inner.blob.branch().id()
            && self.parent_inner.blob.locator() == other.parent_inner.blob.locator()
            && self.name == other.name
    }
}

impl Eq for RefInner<'_> {}