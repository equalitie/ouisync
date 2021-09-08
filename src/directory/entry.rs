use super::{
    inner::{EntryData, Inner},
    parent_context::ParentContext,
    Directory,
};
use crate::{
    blob_id::BlobId,
    entry_type::EntryType,
    error::{Error, Result},
    file::File,
    locator::Locator,
    replica_id::ReplicaId,
    version_vector::VersionVector,
};
use std::{fmt, sync::Arc};

/// Info about a directory entry.
#[derive(Copy, Clone, Debug)]
pub enum EntryRef<'a> {
    File(FileRef<'a>),
    Directory(DirectoryRef<'a>),
}

impl<'a> EntryRef<'a> {
    pub(super) fn new(
        parent_outer: &'a Directory,
        parent_inner: &'a Inner,
        name: &'a str,
        entry_data: &'a EntryData,
        author: &'a ReplicaId,
    ) -> Self {
        let inner = RefInner {
            parent_outer,
            parent_inner,
            entry_data,
            name,
            author,
        };

        match entry_data.entry_type {
            EntryType::File => Self::File(FileRef { inner }),
            EntryType::Directory => Self::Directory(DirectoryRef { inner }),
        }
    }

    pub fn name(&self) -> &'a str {
        match self {
            Self::File(r) => r.name(),
            Self::Directory(r) => r.name(),
        }
    }

    pub fn entry_type(&self) -> EntryType {
        match self {
            Self::File(_) => EntryType::File,
            Self::Directory(_) => EntryType::Directory,
        }
    }

    pub fn blob_id(&self) -> &BlobId {
        &self.inner().entry_data.blob_id
    }

    pub fn version_vector(&self) -> &VersionVector {
        &self.inner().entry_data.version_vector
    }

    pub fn locator(&self) -> Locator {
        Locator::Head(*self.blob_id())
    }

    pub fn file(self) -> Result<FileRef<'a>> {
        match self {
            Self::File(r) => Ok(r),
            Self::Directory(_) => Err(Error::EntryIsDirectory),
        }
    }

    pub fn directory(self) -> Result<DirectoryRef<'a>> {
        match self {
            Self::Directory(r) => Ok(r),
            Self::File(_) => Err(Error::EntryNotDirectory),
        }
    }

    pub fn is_file(&self) -> bool {
        matches!(self, Self::File(_))
    }

    pub fn is_directory(&self) -> bool {
        matches!(self, Self::Directory(_))
    }

    pub fn is_local(&self) -> bool {
        self.inner().is_local()
    }

    fn inner(&self) -> &RefInner {
        match self {
            Self::File(r) => &r.inner,
            Self::Directory(r) => &r.inner,
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub struct FileRef<'a> {
    inner: RefInner<'a>,
}

impl<'a> FileRef<'a> {
    pub fn name(&self) -> &'a str {
        self.inner.name
    }

    pub fn locator(&self) -> Locator {
        Locator::Head(self.inner.entry_data.blob_id)
    }

    pub fn author(&self) -> &'a ReplicaId {
        self.inner.author
    }

    pub fn version_vector(&self) -> &VersionVector {
        &self.inner.entry_data.version_vector
    }

    pub async fn open(&self) -> Result<File> {
        let mut guard = self.inner.entry_data.blob_core.lock().await;
        let blob_core = &mut *guard;

        if let Some(blob_core) = blob_core.upgrade() {
            File::reopen(
                blob_core,
                self.inner.parent_outer.local_branch.clone(),
                self.inner.parent_context(),
            )
            .await
        } else {
            let file = File::open(
                self.inner.parent_inner.blob.branch().clone(),
                self.inner.parent_outer.local_branch.clone(),
                self.locator(),
                self.inner.parent_context(),
            )
            .await?;

            *blob_core = Arc::downgrade(file.blob_core());

            Ok(file)
        }
    }

    pub fn is_local(&self) -> bool {
        self.inner.is_local()
    }
}

impl fmt::Debug for FileRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("FileRef")
            .field("name", &self.inner.name)
            .field("author", &self.inner.author)
            .field("vv", &self.inner.entry_data.version_vector)
            .field("locator", &self.locator())
            .finish()
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub struct DirectoryRef<'a> {
    inner: RefInner<'a>,
}

impl<'a> DirectoryRef<'a> {
    pub fn name(&self) -> &'a str {
        self.inner.name
    }

    pub fn locator(&self) -> Locator {
        Locator::Head(self.inner.entry_data.blob_id)
    }

    pub async fn open(&self) -> Result<Directory> {
        self.inner
            .parent_inner
            .open_directories
            .open(
                self.inner.parent_inner.blob.branch().clone(),
                self.inner.parent_outer.local_branch.clone(),
                self.locator(),
                self.inner.parent_context(),
            )
            .await
    }

    pub fn is_local(&self) -> bool {
        self.inner.is_local()
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
struct RefInner<'a> {
    parent_outer: &'a Directory,
    parent_inner: &'a Inner,
    entry_data: &'a EntryData,
    name: &'a str,
    author: &'a ReplicaId,
}

impl RefInner<'_> {
    fn parent_context(&self) -> ParentContext {
        ParentContext {
            directory: self.parent_outer.clone(),
            entry_name: self.name.into(),
            entry_author: *self.author,
        }
    }

    pub fn is_local(&self) -> bool {
        self.parent_inner.blob.branch().id() == self.parent_outer.local_branch.id()
    }
}

impl PartialEq for RefInner<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.parent_inner.blob.branch().id() == other.parent_inner.blob.branch().id()
            && self.parent_inner.blob.locator() == other.parent_inner.blob.locator()
            && self.entry_data == other.entry_data
            && self.name == other.name
    }
}

impl Eq for RefInner<'_> {}
