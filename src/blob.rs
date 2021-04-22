use crate::{
    block::{self, BlockId, BlockName, BlockVersion, BLOCK_SIZE},
    crypto::{
        aead::{AeadInPlace, NewAead},
        AuthTag, Cipher, Nonce, NonceSequence, SecretKey,
    },
    db,
    error::Result,
    index::{self, BlockKind, ChildTag},
};
use std::{
    convert::TryInto,
    ops::{Deref, DerefMut},
};
use zeroize::Zeroize;

pub struct Blob {
    context: Context,
    nonce_sequence: NonceSequence,
    current_block: OpenBlock,
    len: u64,
}

// TODO: figure out how to implement `flush` on `Drop`.

impl Blob {
    /// Opens an existing blob.
    ///
    /// - `directory_name` is the name of the head block of the directory containing the blob.
    ///   `None` if the blob is the root blob.
    /// - `directory_seq` is the sequence number of the blob within its directory.
    pub async fn open(
        pool: db::Pool,
        secret_key: SecretKey,
        directory_name: Option<BlockName>,
        directory_seq: u32,
    ) -> Result<Self> {
        let context = Context {
            pool,
            secret_key,
            directory_name,
            directory_seq,
        };

        let (id, buffer, auth_tag) = context.load_block(None, 0).await?;

        let mut content = Cursor::new(buffer);

        let nonce_sequence = NonceSequence::with_prefix(content.read_array());
        let nonce = nonce_sequence.get(0);

        context.decrypt_block(&id, &mut content, &auth_tag, &nonce)?;

        let len = content.read_u64();

        let current_block = OpenBlock {
            head_name: id.name,
            number: 0,
            id,
            content,
            dirty: false,
        };

        Ok(Self {
            context,
            nonce_sequence,
            current_block,
            len,
        })
    }

    /// Creates a new blob.
    ///
    /// See [`Self::open`] for explanation of `directory_name` and `directory_seq`.
    pub fn create(
        pool: db::Pool,
        secret_key: SecretKey,
        directory_name: Option<BlockName>,
        directory_seq: u32,
    ) -> Self {
        let context = Context {
            pool,
            secret_key,
            directory_name,
            directory_seq,
        };

        let nonce_sequence = NonceSequence::random();
        let mut content = Cursor::new(Buffer::new());

        content.write(&nonce_sequence.prefix()[..]);
        content.write_u64(0); // blob length

        let id = BlockId::random();
        let current_block = OpenBlock {
            head_name: id.name,
            number: 0,
            id,
            content,
            dirty: true,
        };

        Self {
            context,
            nonce_sequence,
            current_block,
            len: 0,
        }
    }

    /// Length of this blob in bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Reads data from this blob into `buffer`, advancing the internal cursor. Returns the
    /// number of bytes actually read which might be less than `buffer.len()` if the portion of the
    /// blob past the internal cursor is smaller than `buffer.len()`.
    pub async fn read(&mut self, mut buffer: &mut [u8]) -> Result<usize> {
        let mut total_len = 0;

        loop {
            let remaining = (self.len() - self.seek_position())
                .try_into()
                .unwrap_or(usize::MAX);
            let len = buffer.len().min(remaining);
            let len = self.current_block.content.read(&mut buffer[..len]);

            buffer = &mut buffer[len..];
            total_len += len;

            if buffer.is_empty() {
                break;
            }

            let number = self.current_block.next_number();
            if number >= self.block_count() {
                break;
            }

            let (id, content) = self.read_block(number).await?;

            self.replace_current_block(number, id, content).await?;
        }

        Ok(total_len)
    }

    /// Writes `buffer` into this blob, advancing the blob's internal cursor.
    pub async fn write(&mut self, mut buffer: &[u8]) -> Result<()> {
        // TODO: do all the db writes in a transaction

        loop {
            let len = self.current_block.content.write(buffer);

            if len > 0 {
                self.current_block.dirty = true;
            }

            buffer = &buffer[len..];

            self.len = self.len.max(self.seek_position());

            if buffer.is_empty() {
                break;
            }

            let number = self.current_block.next_number();
            let (id, content) = if number < self.block_count() {
                self.read_block(number).await?
            } else {
                (BlockId::random(), Buffer::new())
            };

            self.replace_current_block(number, id, content).await?;
        }

        Ok(())
    }

    /// Flushes this blob, ensuring that all intermediately buffered contents gets written to the
    /// store.
    pub async fn flush(&mut self) -> Result<()> {
        self.write_len().await?;

        if self.current_block.dirty {
            self.write_block(
                &self.current_block.id,
                self.current_block.number,
                self.current_block.content.buffer.clone(),
            )
            .await?;
            self.current_block.dirty = false;
        }

        Ok(())
    }

    async fn replace_current_block(
        &mut self,
        number: u32,
        id: BlockId,
        content: Buffer,
    ) -> Result<()> {
        // TODO: support head block as well
        assert!(number > 0);

        self.flush().await?;

        self.current_block = OpenBlock {
            head_name: self.current_block.head_name,
            number,
            id,
            content: Cursor::new(content),
            dirty: false,
        };

        Ok(())
    }

    async fn read_block(&self, number: u32) -> Result<(BlockId, Buffer)> {
        self.context
            .read_block(
                Some(&self.current_block.head_name),
                number,
                &self.nonce_sequence,
            )
            .await
    }

    async fn write_block(&self, id: &BlockId, number: u32, buffer: Buffer) -> Result<()> {
        self.context
            .write_block(
                &self.current_block.head_name,
                number,
                &self.nonce_sequence,
                id,
                buffer,
            )
            .await
    }

    // Write the current blob length into the blob header in the head block.
    async fn write_len(&mut self) -> Result<()> {
        if self.current_block.number == 0 {
            let old_pos = self.current_block.content.pos;
            self.current_block.content.pos = self.nonce_sequence.prefix().len();
            self.current_block.content.write_u64(self.len);
            self.current_block.content.pos = old_pos;
        } else {
            let (mut id, buffer) = self.read_block(0).await?;
            let mut cursor = Cursor::new(buffer);
            cursor.pos = self.nonce_sequence.prefix().len();
            cursor.write_u64(self.len());
            id.version = BlockVersion::random();
            self.write_block(&id, 0, cursor.buffer).await?;
        }

        Ok(())
    }

    // Total number of blocks in this blob including the possibly partially filled final block.
    fn block_count(&self) -> u32 {
        // https://stackoverflow.com/questions/2745074/fast-ceiling-of-an-integer-division-in-c-c
        // NOTE: when `len()` is zero this still returns 1 which is actually correct in this case
        // because even empty blob needs one block to store the nonce prefix and the blob length.
        (1 + (self.len().saturating_sub(1)) / BLOCK_SIZE as u64)
            .try_into()
            .unwrap_or(u32::MAX)
    }

    // Returns the current seek position from the start of the blob.
    fn seek_position(&self) -> u64 {
        self.current_block.number as u64 * BLOCK_SIZE as u64 + self.current_block.content.pos as u64
    }
}

struct Context {
    pool: db::Pool,
    secret_key: SecretKey,
    directory_name: Option<BlockName>,
    directory_seq: u32,
}

impl Context {
    async fn read_block(
        &self,
        head_name: Option<&BlockName>,
        number: u32,
        nonce_sequence: &NonceSequence,
    ) -> Result<(BlockId, Buffer)> {
        let (id, mut buffer, auth_tag) = self.load_block(head_name, number).await?;

        let nonce = nonce_sequence.get(number);
        let offset = if number == 0 {
            nonce_sequence.prefix().len()
        } else {
            0
        };

        self.decrypt_block(&id, &mut buffer[offset..], &auth_tag, &nonce)?;

        Ok((id, buffer))
    }

    async fn load_block(
        &self,
        head_name: Option<&BlockName>,
        number: u32,
    ) -> Result<(BlockId, Buffer, AuthTag)> {
        let id = if let Some(child_tag) = self.child_tag(head_name, number) {
            index::get(&self.pool, &child_tag).await?
        } else {
            index::get_root(&self.pool).await?
        };

        let mut content = Buffer::new();
        let auth_tag = block::read(&self.pool, &id, &mut content).await?;

        Ok((id, content, auth_tag))
    }

    fn decrypt_block(
        &self,
        id: &BlockId,
        buffer: &mut [u8],
        auth_tag: &AuthTag,
        nonce: &Nonce,
    ) -> Result<()> {
        let aad = id.to_array(); // "additional associated data"
        let cipher = Cipher::new(self.secret_key.as_array());
        cipher.decrypt_in_place_detached(&nonce, &aad, buffer, &auth_tag)?;

        Ok(())
    }

    async fn write_block(
        &self,
        head_name: &BlockName,
        number: u32,
        nonce_sequence: &NonceSequence,
        id: &BlockId,
        mut buffer: Buffer,
    ) -> Result<()> {
        let nonce = nonce_sequence.get(number);
        let aad = id.to_array(); // "additional associated data"

        let offset = if number == 0 {
            nonce_sequence.prefix().len()
        } else {
            0
        };

        let cipher = Cipher::new(self.secret_key.as_array());
        let auth_tag = cipher.encrypt_in_place_detached(&nonce, &aad, &mut buffer[offset..])?;

        block::write(&self.pool, id, &buffer, &auth_tag).await?;

        if let Some(child_tag) = self.child_tag(Some(head_name), number) {
            index::insert(&self.pool, id, &child_tag).await?;
        } else {
            index::insert_root(&self.pool, id).await?;
        }

        Ok(())
    }

    fn child_tag(&self, head_name: Option<&BlockName>, number: u32) -> Option<ChildTag> {
        match (number, &self.directory_name) {
            (0, None) => None, // root
            (0, Some(directory_name)) => Some(ChildTag::new(
                &self.secret_key,
                directory_name,
                self.directory_seq,
                BlockKind::Head,
            )),
            (_, _) => Some(ChildTag::new(
                &self.secret_key,
                head_name.expect("head name is required for trunk blocks"),
                number,
                BlockKind::Trunk,
            )),
        }
    }
}

// Data for a block that's been loaded into memory and decrypted.
struct OpenBlock {
    // Name of the head block of the blob. If this `OpenBlock` represents the head block, this is
    // the same as `id.name`.
    head_name: BlockName,
    // Number of this blob within the blob. Head block's number is 0, then next one is 1, and so
    // on...
    number: u32,
    // Id of the block.
    id: BlockId,
    // Decrypted content of the block wrapped in `Cursor` to track the current seek position.
    content: Cursor,
    // Was this block modified since the last time it was loaded from/saved to the store?
    dirty: bool,
}

impl OpenBlock {
    fn next_number(&self) -> u32 {
        // TODO: return error instead of panic
        self.number
            .checked_add(1)
            .expect("block count limit exceeded")
    }
}

// Buffer for keeping loaded block content and also for in-place encryption and decryption.
#[derive(Clone)]
struct Buffer(Box<[u8]>);

impl Buffer {
    fn new() -> Self {
        Self(vec![0; BLOCK_SIZE].into_boxed_slice())
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
struct Cursor {
    buffer: Buffer,
    pos: usize,
}

impl Cursor {
    fn new(buffer: Buffer) -> Self {
        Self { buffer, pos: 0 }
    }

    // Reads data from the buffer into `dst` and advances the internal position. Returns the
    // number of bytes actual read.
    fn read(&mut self, dst: &mut [u8]) -> usize {
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
        let array = self.buffer[self.pos..self.pos + N].try_into().unwrap();
        self.pos += N;
        array
    }

    // Read data from the buffer into a `u64`.
    //
    // # Panics
    //
    // Panics if the remaining length is less than `size_of::<u64>()`
    fn read_u64(&mut self) -> u64 {
        u64::from_le_bytes(self.read_array())
    }

    // Writes data from `dst` into the buffer and advances the internal position. Returns the
    // number of bytes actually written.
    fn write(&mut self, src: &[u8]) -> usize {
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
    fn write_u64(&mut self, value: u64) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{distributions::Standard, Rng};

    #[tokio::test(flavor = "multi_thread")]
    async fn root_blob() {
        let pool = init_db().await;
        let secret_key = SecretKey::random();

        // Create a blob spanning 2.5 blocks
        let orig_content: Vec<u8> = rand::thread_rng()
            .sample_iter(Standard)
            .take(5 * BLOCK_SIZE / 2)
            .collect();

        let mut blob = Blob::create(pool.clone(), secret_key.clone(), None, 0);
        blob.write(&orig_content[..]).await.unwrap();
        blob.flush().await.unwrap();
        drop(blob);

        // Re-open the blob and read its contents.
        let mut blob = Blob::open(pool.clone(), secret_key.clone(), None, 0)
            .await
            .unwrap();

        // Read it in chunks of this size.
        let chunk_size = 1024;
        let mut read_contents = vec![0; chunk_size];
        let mut read_len = 0;

        loop {
            let len = blob.read(&mut read_contents[read_len..]).await.unwrap();
            if len == 0 {
                break; // done
            }

            read_len += len;
            read_contents.resize(read_contents.len() + chunk_size, 0);
        }

        assert_eq!(&read_contents[..read_len], &orig_content[..])
    }

    async fn init_db() -> db::Pool {
        let pool = db::Pool::connect(":memory:").await.unwrap();
        index::init(&pool).await.unwrap();
        block::init(&pool).await.unwrap();
        pool
    }
}
