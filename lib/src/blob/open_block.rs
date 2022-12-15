use crate::{
    block::{BlockId, BLOCK_SIZE},
    crypto::cipher,
    db,
    error::Result,
    index::SnapshotData,
    locator::Locator,
};
use std::{
    convert::TryInto,
    ops::{Deref, DerefMut},
};
use zeroize::Zeroize;

// Data for a block that's been loaded into memory and decrypted.
#[derive(Clone)]
pub(super) struct OpenBlock {
    // Locator of the block.
    pub locator: Locator,
    // Id of the block.
    pub id: BlockId,
    // Decrypted content of the block wrapped in `Cursor` to track the current seek position.
    pub content: Cursor,
    // Was this block modified since the last time it was loaded from/saved to the store?
    pub dirty: bool,
}

impl OpenBlock {
    pub fn new_head(locator: Locator) -> Self {
        let mut content = Cursor::new(Buffer::new());
        content.write_u64(0); // blob length (initially zero)

        Self {
            locator,
            id: BlockId::from_content(&[]),
            content,
            dirty: true,
        }
    }

    pub async fn open_head(
        conn: &mut db::Connection,
        snapshot: &SnapshotData,
        read_key: &cipher::SecretKey,
        locator: Locator,
    ) -> Result<Self> {
        let (id, buffer) = super::read_block(conn, snapshot, read_key, &locator).await?;
        let content = Cursor::new(buffer);

        Ok(Self {
            locator,
            id,
            content,
            dirty: false,
        })
    }
}

// Buffer for keeping loaded block content and also for in-place encryption and decryption.
#[derive(Clone)]
pub(super) struct Buffer(Box<[u8]>);

impl Buffer {
    pub fn new() -> Self {
        Self(vec![0; BLOCK_SIZE].into_boxed_slice())
    }

    // Read data from `offset` of the buffer into a fixed-length array.
    //
    // # Panics
    //
    // Panics if the remaining length after `offset` is less than `N`.
    pub fn read_array<const N: usize>(&self, offset: usize) -> [u8; N] {
        self[offset..offset + N].try_into().unwrap()
    }
}

// Scramble the buffer on drop to prevent leaving decrypted data in memory past the buffer
// lifetime.
impl Drop for Buffer {
    fn drop(&mut self) {
        self.0.zeroize()
    }
}

impl Deref for Buffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Buffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// Wrapper for `Buffer` with an internal position which advances when data is read from or
// written to the buffer.
#[derive(Clone)]
pub(super) struct Cursor {
    pub buffer: Buffer,
    pub pos: usize,
}

impl Cursor {
    pub fn new(buffer: Buffer) -> Self {
        Self { buffer, pos: 0 }
    }

    // Reads data from the buffer into `dst` and advances the internal position. Returns the
    // number of bytes actual read.
    pub fn read(&mut self, dst: &mut [u8]) -> usize {
        let n = (self.buffer.len() - self.pos).min(dst.len());
        dst[..n].copy_from_slice(&self.buffer[self.pos..self.pos + n]);
        self.pos += n;
        n
    }

    // Read data from the buffer into a fixed-length array.
    //
    // # Panics
    //
    // Panics if the remaining length is less than `N`.
    fn read_array<const N: usize>(&mut self) -> [u8; N] {
        let array = self.buffer.read_array(self.pos);
        self.pos += N;
        array
    }

    // Read data from the buffer into a `u64`.
    //
    // # Panics
    //
    // Panics if the remaining length is less than `size_of::<u64>()`
    pub fn read_u64(&mut self) -> u64 {
        u64::from_le_bytes(self.read_array())
    }

    // Writes data from `dst` into the buffer and advances the internal position. Returns the
    // number of bytes actually written.
    pub fn write(&mut self, src: &[u8]) -> usize {
        let n = (self.buffer.len() - self.pos).min(src.len());
        self.buffer[self.pos..self.pos + n].copy_from_slice(&src[..n]);
        self.pos += n;
        n
    }

    // Write a `u64` into the buffer.
    //
    // # Panics
    //
    // Panics if the remaining length is less than `size_of::<u64>()`
    pub fn write_u64(&mut self, value: u64) {
        let bytes = value.to_le_bytes();
        assert!(self.buffer.len() - self.pos >= bytes.len());
        self.write(&bytes[..]);
    }
}

impl Deref for Cursor {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.buffer[self.pos..]
    }
}

impl DerefMut for Cursor {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.buffer[self.pos..]
    }
}
