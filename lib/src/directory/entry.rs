use super::{
    content::Content,
    entry_data::{EntryData, EntryDirectoryData, EntryFileData, EntryTombstoneData},
    parent_context::ParentContext,
    Directory, DirectoryFallback, DirectoryLocking,
};
use crate::{
    blob::BlobId,
    branch::Branch,
    crypto::sign::PublicKey,
    error::{Error, Result},
    file::File,
    protocol::Locator,
    store::ReadTransaction,
    version_vector::VersionVector,
    versioned::{BranchItem, Versioned},
};
use std::fmt;

/// Info about a directory entry.
#[derive(Copy, Clone, Debug)]
pub enum EntryRef<'a> {
    File(FileRef<'a>),
    Directory(DirectoryRef<'a>),
    Tombstone(TombstoneRef<'a>),
}

impl<'a> EntryRef<'a> {
    pub(super) fn new(parent: &'a Directory, name: &'a str, entry_data: &'a EntryData) -> Self {
        let inner = RefInner { parent, name };

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

    pub fn version_vector(&self) -> &'a VersionVector {
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
        self.inner().parent
    }

    pub(crate) fn clone_data(&self) -> EntryData {
        match self {
            Self::File(e) => EntryData::File(e.data().clone()),
            Self::Directory(e) => EntryData::Directory(e.data().clone()),
            Self::Tombstone(e) => EntryData::Tombstone(e.data().clone()),
        }
    }

    fn inner(&self) -> &RefInner<'_> {
        match self {
            Self::File(r) => &r.inner,
            Self::Directory(r) => &r.inner,
            Self::Tombstone(r) => &r.inner,
        }
    }
}

impl Versioned for EntryRef<'_> {
    fn version_vector(&self) -> &VersionVector {
        EntryRef::version_vector(self)
    }
}

impl BranchItem for EntryRef<'_> {
    fn branch_id(&self) -> &PublicKey {
        EntryRef::branch_id(self)
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

    pub(crate) fn blob_id(&self) -> &'a BlobId {
        &self.entry_data.blob_id
    }

    pub fn version_vector(&self) -> &'a VersionVector {
        &self.entry_data.version_vector
    }

    pub async fn open(&self) -> Result<File> {
        let parent_context = self.inner.parent_context();
        let branch = self.branch().clone();
        let locator = self.locator();

        File::open(branch, locator, parent_context).await
    }

    /// Fork the file without opening it.
    pub(crate) async fn fork(&self, dst_branch: &Branch) -> Result<()> {
        if self.branch().id() == dst_branch.id() {
            // Already forked
            return Ok(());
        }

        let parent_context = self.inner.parent_context();
        let src_branch = self.branch();

        parent_context.fork(src_branch, dst_branch).await?;

        Ok(())
    }

    pub fn branch(&self) -> &Branch {
        self.inner.branch()
    }

    pub fn parent(&self) -> &Directory {
        self.inner.parent
    }

    pub(crate) fn data(&self) -> &EntryFileData {
        self.entry_data
    }
}

impl fmt::Debug for FileRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("FileRef")
            .field("name", &self.inner.name)
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
        Locator::head(*self.blob_id())
    }

    pub(crate) fn blob_id(&self) -> &'a BlobId {
        &self.entry_data.blob_id
    }

    pub(crate) async fn open(&self, fallback: DirectoryFallback) -> Result<Directory> {
        Directory::open(
            self.branch().clone(),
            *self.blob_id(),
            Some(self.inner.parent_context()),
            if self.inner.parent.lock.is_some() {
                DirectoryLocking::Enabled
            } else {
                DirectoryLocking::Disabled
            },
            fallback,
        )
        .await
    }

    pub(super) async fn open_snapshot(
        &self,
        tx: &mut ReadTransaction,
        fallback: DirectoryFallback,
    ) -> Result<Content> {
        Directory::open_snapshot(tx, self.branch().clone(), self.locator(), fallback).await
    }

    pub fn branch(&self) -> &'a Branch {
        self.inner.branch()
    }

    pub(crate) fn data(&self) -> &EntryDirectoryData {
        self.entry_data
    }

    pub fn version_vector(&self) -> &'a VersionVector {
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

    pub(crate) fn data(&self) -> &EntryTombstoneData {
        self.entry_data
    }

    pub fn version_vector(&self) -> &'a VersionVector {
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
    parent: &'a Directory,
    name: &'a str,
}

impl<'a> RefInner<'a> {
    fn parent_context(&self) -> ParentContext {
        self.parent.create_parent_context(self.name.into())
    }

    fn branch(&self) -> &'a Branch {
        self.parent.blob.branch()
    }

    pub fn branch_id(&self) -> &'a PublicKey {
        self.parent.blob.branch().id()
    }
}

impl PartialEq for RefInner<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.parent.blob.branch().id() == other.parent.blob.branch().id()
            && self.parent.blob.id() == other.parent.blob.id()
            && self.name == other.name
    }
}

impl Eq for RefInner<'_> {}
