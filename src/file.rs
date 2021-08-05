use crate::{
    blob::Blob, branch::Branch, entry_type::EntryType, error::Result, locator::Locator,
    write_context::WriteContext,
};
use camino::Utf8PathBuf;
use std::io::SeekFrom;

pub struct File {
    blob: Blob,
    write_context: WriteContext,
}

impl File {
    /// Opens an existing file.
    pub async fn open(
        branch: Branch,
        locator: Locator,
        write_context: WriteContext,
    ) -> Result<Self> {
        Ok(Self {
            blob: Blob::open(branch, locator).await?,
            write_context,
        })
    }

    /// Creates a new file.
    pub fn create(branch: Branch, locator: Locator, path: Utf8PathBuf) -> Self {
        Self {
            blob: Blob::create(branch.clone(), locator),
            write_context: WriteContext::new(path, branch),
        }
    }

    pub fn branch(&self) -> &Branch {
        self.blob.branch()
    }

    /// Length of this file in bytes.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> u64 {
        self.blob.len()
    }

    /// Locator of this file.
    pub fn locator(&self) -> &Locator {
        self.blob.locator()
    }

    /// Reads data from this file. See [`Blob::read`] for more info.
    pub async fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        self.blob.read(buffer).await
    }

    /// Read all data from this file from the current seek position until the end and return then
    /// in a `Vec`.
    pub async fn read_to_end(&mut self) -> Result<Vec<u8>> {
        self.blob.read_to_end().await
    }

    /// Writes `buffer` into this file.
    pub async fn write(&mut self, buffer: &[u8]) -> Result<()> {
        if !buffer.is_empty() {
            self.write_context
                .begin(EntryType::File, &mut self.blob)
                .await?;
        }

        self.blob.write(buffer).await
    }

    /// Seeks to an offset in the file.
    pub async fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.blob.seek(pos).await
    }

    /// Truncates the file to the given length.
    pub async fn truncate(&mut self, len: u64) -> Result<()> {
        if len != self.blob.len() {
            self.write_context
                .begin(EntryType::File, &mut self.blob)
                .await?;
        }

        self.blob.truncate(len).await
    }

    /// Flushes this file, ensuring that all intermediately buffered contents gets written to the
    /// store.
    pub async fn flush(&mut self) -> Result<()> {
        self.blob.flush().await?;
        self.write_context.commit().await
    }

    /// Removes this file.
    pub async fn remove(self) -> Result<()> {
        // TODO: consider only allowing this if file is in the local branch.
        self.blob.remove().await
    }
}
