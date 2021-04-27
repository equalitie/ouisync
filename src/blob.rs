use crate::{
    block::{self, BlockId, BlockName, BlockVersion, BLOCK_SIZE},
    crypto::{
        aead::{AeadInPlace, NewAead},
        AuthTag, Cipher, Nonce, NonceSequence, SecretKey,
    },
    db,
    error::Result,
    index,
    locator::Locator,
};
use std::{
    convert::TryInto,
    io::SeekFrom,
    mem,
    ops::{Deref, DerefMut},
};
use zeroize::Zeroize;

pub struct Blob {
    pool: db::Pool,
    locator: Locator,
    secret_key: SecretKey,
    nonce_sequence: NonceSequence,
    current_block: OpenBlock,
    len: u64,
    len_dirty: bool,
}

// TODO: figure out how to implement `flush` on `Drop`.

impl Blob {
    /// Opens an existing blob.
    ///
    /// - `directory_name` is the name of the head block of the directory containing the blob.
    ///   `None` if the blob is the root blob.
    /// - `directory_seq` is the sequence number of the blob within its directory.
    pub(crate) async fn open(
        pool: db::Pool,
        secret_key: SecretKey,
        locator: Locator,
    ) -> Result<Self> {
        // NOTE: no need to commit this transaction because we are only reading here.
        let mut tx = pool.begin().await?;
        let (id, buffer, auth_tag) = load_block(&mut tx, &secret_key, &locator).await?;

        let mut content = Cursor::new(buffer);

        let nonce_sequence = NonceSequence::with_prefix(content.read_array());
        let nonce = nonce_sequence.get(0);

        decrypt_block(&secret_key, &nonce, &id, &mut content, &auth_tag)?;

        let len = content.read_u64();

        let current_block = OpenBlock {
            locator,
            id,
            content,
            dirty: false,
        };

        Ok(Self {
            pool,
            locator,
            secret_key,
            nonce_sequence,
            current_block,
            len,
            len_dirty: false,
        })
    }

    /// Creates a new blob.
    ///
    /// See [`Self::open`] for explanation of `directory_name` and `directory_seq`.
    pub(crate) fn create(pool: db::Pool, secret_key: SecretKey, locator: Locator) -> Self {
        let nonce_sequence = NonceSequence::random();
        let mut content = Cursor::new(Buffer::new());

        content.write(&nonce_sequence.prefix()[..]);
        content.write_u64(0); // blob length

        let id = BlockId::random();
        let current_block = OpenBlock {
            locator,
            id,
            content,
            dirty: true,
        };

        Self {
            pool,
            locator,
            secret_key,
            nonce_sequence,
            current_block,
            len: 0,
            len_dirty: false,
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

            let locator = self.next_locator();
            if locator.number() >= self.block_count() {
                break;
            }

            // NOTE: unlike in `write` we create a separate transaction for each iteration. This is
            // because if we created a single transaction for the whole `read` call, then a failed
            // read could rollback the changes made in a previous iteration which would then be
            // lost. This is fine because there is going to be at most one dirty block within
            // a single `read` invocation anyway.
            let mut tx = self.pool.begin().await?;

            let (id, content) =
                read_block(&mut tx, &self.secret_key, &self.nonce_sequence, &locator).await?;

            self.replace_current_block(&mut tx, locator, id, content)
                .await?;

            tx.commit().await?;
        }

        Ok(total_len)
    }

    /// Read all data from this blob from the current seek position until the end and return then
    /// in a `Vec`.
    pub async fn read_to_end(&mut self) -> Result<Vec<u8>> {
        let mut buffer = vec![
            0;
            (self.len() - self.seek_position())
                .try_into()
                .unwrap_or(usize::MAX)
        ];

        let len = self.read(&mut buffer).await?;
        buffer.truncate(len);

        Ok(buffer)
    }

    /// Writes `buffer` into this blob, advancing the blob's internal cursor.
    pub async fn write(&mut self, mut buffer: &[u8]) -> Result<()> {
        // Wrap the whole `write` in a transaction to make it atomic.
        let mut tx = self.pool.begin().await?;

        loop {
            let len = self.current_block.content.write(buffer);

            // TODO: only set the dirty flag if the content actually changed. Otherwise overwirting
            // a block with the same content it already had would result in a new block with a new
            // version being unnecessarily created.
            if len > 0 {
                self.current_block.dirty = true;
            }

            buffer = &buffer[len..];

            if self.seek_position() > self.len {
                self.len = self.seek_position();
                self.len_dirty = true;
            }

            if buffer.is_empty() {
                break;
            }

            let locator = self.next_locator();
            let (id, content) = if locator.number() < self.block_count() {
                read_block(&mut tx, &self.secret_key, &self.nonce_sequence, &locator).await?
            } else {
                (BlockId::random(), Buffer::new())
            };

            self.replace_current_block(&mut tx, locator, id, content)
                .await?;
        }

        tx.commit().await?;

        Ok(())
    }

    /// Seek to an offset in the blob.
    ///
    /// It is allowed to specify offset that is outside of the range of the blob but such offset
    /// will be clamped to be within the range.
    ///
    /// Returns the new seek position from the start of the blob.
    pub async fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let offset = match pos {
            SeekFrom::Start(n) => n.min(self.len),
            SeekFrom::End(n) => {
                if n >= 0 {
                    self.len
                } else {
                    self.len.saturating_sub((-n) as u64)
                }
            }
            SeekFrom::Current(n) => {
                if n >= 0 {
                    self.seek_position().saturating_add(n as u64).min(self.len)
                } else {
                    self.seek_position().saturating_sub((-n) as u64)
                }
            }
        };

        let actual_offset = offset + self.header_size() as u64;
        let block_number = (actual_offset / BLOCK_SIZE as u64) as u32;
        let block_offset = (actual_offset % BLOCK_SIZE as u64) as usize;

        if block_number != self.current_block.locator.number() {
            let locator = self.locator_at(block_number);

            let mut tx = self.pool.begin().await?;
            let (id, content) =
                read_block(&mut tx, &self.secret_key, &self.nonce_sequence, &locator).await?;
            self.replace_current_block(&mut tx, locator, id, content)
                .await?;
            tx.commit().await?;
        }

        self.current_block.content.pos = block_offset;

        Ok(offset)
    }

    /// Truncate the blob to zero length.
    pub async fn truncate(&mut self) -> Result<()> {
        todo!()
    }

    /// Flushes this blob, ensuring that all intermediately buffered contents gets written to the
    /// store.
    pub async fn flush(&mut self) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.flush_in(&mut tx).await?;
        tx.commit().await?;

        Ok(())
    }

    pub(crate) fn head_name(&self) -> &BlockName {
        self.current_block.head_name()
    }

    pub(crate) fn db_pool(&self) -> &db::Pool {
        &self.pool
    }

    pub(crate) fn secret_key(&self) -> &SecretKey {
        &self.secret_key
    }

    async fn flush_in(&mut self, tx: &mut db::Transaction) -> Result<()> {
        if !self.current_block.dirty {
            return Ok(());
        }

        self.current_block.id.version = BlockVersion::random();

        self.write_len(tx).await?;
        write_block(
            tx,
            &self.secret_key,
            &self.nonce_sequence,
            &self.current_block.locator,
            &self.current_block.id,
            self.current_block.content.buffer.clone(),
        )
        .await?;

        self.current_block.dirty = false;

        Ok(())
    }

    async fn replace_current_block(
        &mut self,
        tx: &mut db::Transaction,
        locator: Locator,
        id: BlockId,
        content: Buffer,
    ) -> Result<()> {
        self.flush_in(tx).await?;

        let mut content = Cursor::new(content);

        if locator.number() == 0 {
            // If head block, skip over the header.
            content.pos = self.header_size();
        }

        self.current_block = OpenBlock {
            locator,
            id,
            content,
            dirty: false,
        };

        Ok(())
    }

    // Write the current blob length into the blob header in the head block.
    async fn write_len(&mut self, tx: &mut db::Transaction) -> Result<()> {
        if !self.len_dirty {
            return Ok(());
        }

        if self.current_block.locator.number() == 0 {
            let old_pos = self.current_block.content.pos;
            self.current_block.content.pos = self.nonce_sequence.prefix().len();
            self.current_block.content.write_u64(self.len);
            self.current_block.content.pos = old_pos;
            self.current_block.dirty = true;
        } else {
            let locator = self.locator_at(0);
            let (mut id, buffer) =
                read_block(tx, &self.secret_key, &self.nonce_sequence, &locator).await?;

            let mut cursor = Cursor::new(buffer);
            cursor.pos = self.nonce_sequence.prefix().len();
            cursor.write_u64(self.len);
            id.version = BlockVersion::random();

            write_block(
                tx,
                &self.secret_key,
                &self.nonce_sequence,
                &locator,
                &id,
                cursor.buffer,
            )
            .await?;
        }

        self.len_dirty = false;

        Ok(())
    }

    // Total number of blocks in this blob including the possibly partially filled final block.
    fn block_count(&self) -> u32 {
        // https://stackoverflow.com/questions/2745074/fast-ceiling-of-an-integer-division-in-c-c
        (1 + (self.len + self.header_size() as u64 - 1) / BLOCK_SIZE as u64)
            .try_into()
            .unwrap_or(u32::MAX)
    }

    // Returns the current seek position from the start of the blob.
    fn seek_position(&self) -> u64 {
        self.current_block.locator.number() as u64 * BLOCK_SIZE as u64
            + self.current_block.content.pos as u64
            - self.header_size() as u64
    }

    fn header_size(&self) -> usize {
        self.nonce_sequence.prefix().len() + mem::size_of_val(&self.len)
    }

    fn locator_at(&self, number: u32) -> Locator {
        if number == 0 {
            self.locator
        } else if let Some(head_name) = self.current_block.locator.head_name() {
            Locator::Trunk(*head_name, number)
        } else {
            Locator::Trunk(self.current_block.id.name, number)
        }
    }

    fn next_locator(&self) -> Locator {
        // TODO: return error instead of panic
        let number = self
            .current_block
            .locator
            .number()
            .checked_add(1)
            .expect("block count limit exceeded");
        self.locator_at(number)
    }
}

async fn read_block(
    tx: &mut db::Transaction,
    secret_key: &SecretKey,
    nonce_sequence: &NonceSequence,
    locator: &Locator,
) -> Result<(BlockId, Buffer)> {
    let (id, mut buffer, auth_tag) = load_block(tx, secret_key, locator).await?;

    let number = locator.number();
    let nonce = nonce_sequence.get(number);
    let offset = if number == 0 {
        nonce_sequence.prefix().len()
    } else {
        0
    };

    decrypt_block(secret_key, &nonce, &id, &mut buffer[offset..], &auth_tag)?;

    Ok((id, buffer))
}

async fn load_block(
    tx: &mut db::Transaction,
    secret_key: &SecretKey,
    locator: &Locator,
) -> Result<(BlockId, Buffer, AuthTag)> {
    let id = if let Some(child_tag) = locator.encode(secret_key) {
        index::get(tx, &child_tag).await?
    } else {
        index::get_root(tx).await?
    };

    let mut content = Buffer::new();
    let auth_tag = block::read(tx, &id, &mut content).await?;

    Ok((id, content, auth_tag))
}

fn decrypt_block(
    secret_key: &SecretKey,
    nonce: &Nonce,
    id: &BlockId,
    buffer: &mut [u8],
    auth_tag: &AuthTag,
) -> Result<()> {
    let aad = id.to_array(); // "additional associated data"
    let cipher = Cipher::new(secret_key.as_array());
    cipher.decrypt_in_place_detached(&nonce, &aad, buffer, &auth_tag)?;

    Ok(())
}

async fn write_block(
    tx: &mut db::Transaction,
    secret_key: &SecretKey,
    nonce_sequence: &NonceSequence,
    locator: &Locator,
    block_id: &BlockId,
    mut buffer: Buffer,
) -> Result<()> {
    let number = locator.number();
    let nonce = nonce_sequence.get(number);
    let aad = block_id.to_array(); // "additional associated data"

    let offset = if number == 0 {
        nonce_sequence.prefix().len()
    } else {
        0
    };

    let cipher = Cipher::new(secret_key.as_array());
    let auth_tag = cipher.encrypt_in_place_detached(&nonce, &aad, &mut buffer[offset..])?;

    block::write(tx, block_id, &buffer, &auth_tag).await?;

    if let Some(child_tag) = locator.encode(secret_key) {
        index::insert(tx, block_id, &child_tag).await?;
    } else {
        index::insert_root(tx, block_id).await?;
    }

    Ok(())
}

// Data for a block that's been loaded into memory and decrypted.
struct OpenBlock {
    // Locator of the block.
    locator: Locator,
    // Id of the block.
    id: BlockId,
    // Decrypted content of the block wrapped in `Cursor` to track the current seek position.
    content: Cursor,
    // Was this block modified since the last time it was loaded from/saved to the store?
    dirty: bool,
}

impl OpenBlock {
    fn head_name(&self) -> &BlockName {
        self.locator.head_name().unwrap_or(&self.id.name)
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
    use proptest::{collection::vec, prelude::*};
    use rand::{distributions::Standard, prelude::*};
    use std::future::Future;
    use test_strategy::proptest;

    #[tokio::test(flavor = "multi_thread")]
    async fn empty_blob() {
        let pool = init_db().await;
        let secret_key = SecretKey::random();

        let mut blob = Blob::create(pool.clone(), secret_key.clone(), Locator::Root);
        blob.flush().await.unwrap();

        // Re-open the blob and read its contents.
        let mut blob = Blob::open(pool.clone(), secret_key.clone(), Locator::Root)
            .await
            .unwrap();

        let mut buffer = [0; 1];
        assert_eq!(blob.read(&mut buffer[..]).await.unwrap(), 0);
    }

    #[proptest]
    fn write_and_read(
        #[strategy(1..3 * BLOCK_SIZE)] blob_len: usize,
        #[strategy(1..=#blob_len)] write_len: usize,
        #[strategy(1..=#blob_len + 1)] read_len: usize,
        #[strategy(rng_seed_strategy())] rng_seed: u64,
    ) {
        run(async {
            let (rng, secret_key, pool) = setup(rng_seed).await;

            // Create the blob and write to it in chunks of `write_len` bytes.
            let mut blob = Blob::create(pool.clone(), secret_key.clone(), Locator::Root);

            let orig_content: Vec<u8> = rng.sample_iter(Standard).take(blob_len).collect();

            for chunk in orig_content.chunks(write_len) {
                blob.write(chunk).await.unwrap();
            }

            blob.flush().await.unwrap();

            // Re-open the blob and read from it in chunks of `read_len` bytes
            let mut blob = Blob::open(pool.clone(), secret_key.clone(), Locator::Root)
                .await
                .unwrap();

            let mut read_content = vec![0; 0];
            let mut read_buffer = vec![0; read_len];

            loop {
                let len = blob.read(&mut read_buffer[..]).await.unwrap();

                if len == 0 {
                    break; // done
                }

                read_content.extend(&read_buffer[..len]);
            }

            assert_eq!(read_content, orig_content);
        })
    }

    #[proptest]
    fn len(
        #[strategy(0..3 * BLOCK_SIZE)] content_len: usize,
        #[strategy(rng_seed_strategy())] rng_seed: u64,
    ) {
        run(async {
            let (rng, secret_key, pool) = setup(rng_seed).await;

            let content: Vec<u8> = rng.sample_iter(Standard).take(content_len).collect();

            let mut blob = Blob::create(pool.clone(), secret_key.clone(), Locator::Root);
            blob.write(&content[..]).await.unwrap();
            assert_eq!(blob.len(), content_len as u64);

            blob.flush().await.unwrap();
            assert_eq!(blob.len(), content_len as u64);

            let blob = Blob::open(pool.clone(), secret_key.clone(), Locator::Root)
                .await
                .unwrap();
            assert_eq!(blob.len(), content_len as u64);
        })
    }

    #[proptest]
    fn seek_from_start(
        #[strategy(0..2 * BLOCK_SIZE)] content_len: usize,
        #[strategy(0..#content_len)] pos: usize,
        #[strategy(rng_seed_strategy())] rng_seed: u64,
    ) {
        run(seek_from(
            content_len,
            SeekFrom::Start(pos as u64),
            pos,
            rng_seed,
        ))
    }

    #[proptest]
    fn seek_from_end(
        #[strategy(0..2 * BLOCK_SIZE)] content_len: usize,
        #[strategy(0..#content_len)] pos: usize,
        #[strategy(rng_seed_strategy())] rng_seed: u64,
    ) {
        run(seek_from(
            content_len,
            SeekFrom::End(-((content_len - pos) as i64)),
            pos,
            rng_seed,
        ))
    }

    async fn seek_from(
        content_len: usize,
        seek_from: SeekFrom,
        expected_pos: usize,
        rng_seed: u64,
    ) {
        let (rng, secret_key, pool) = setup(rng_seed).await;

        let content: Vec<u8> = rng.sample_iter(Standard).take(content_len).collect();

        let mut blob = Blob::create(pool.clone(), secret_key.clone(), Locator::Root);
        blob.write(&content[..]).await.unwrap();
        blob.flush().await.unwrap();

        blob.seek(seek_from).await.unwrap();

        let mut read_buffer = vec![0; content.len()];
        let len = blob.read(&mut read_buffer[..]).await.unwrap();
        assert_eq!(read_buffer[..len], content[expected_pos..]);
    }

    #[proptest]
    fn seek_from_current(
        #[strategy(1..2 * BLOCK_SIZE)] content_len: usize,
        #[strategy(vec(0..#content_len, 1..10))] positions: Vec<usize>,
        #[strategy(rng_seed_strategy())] rng_seed: u64,
    ) {
        run(async {
            let (rng, secret_key, pool) = setup(rng_seed).await;

            let content: Vec<u8> = rng.sample_iter(Standard).take(content_len).collect();

            let mut blob = Blob::create(pool.clone(), secret_key.clone(), Locator::Root);
            blob.write(&content[..]).await.unwrap();
            blob.flush().await.unwrap();
            blob.seek(SeekFrom::Start(0)).await.unwrap();

            let mut prev_pos = 0;
            for pos in positions {
                blob.seek(SeekFrom::Current(pos as i64 - prev_pos as i64))
                    .await
                    .unwrap();
                prev_pos = pos;
            }

            let mut read_buffer = vec![0; content.len()];
            let len = blob.read(&mut read_buffer[..]).await.unwrap();
            assert_eq!(read_buffer[..len], content[prev_pos..]);
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn seek_after_end() {
        let (_, secret_key, pool) = setup(0).await;

        let content = b"content";

        let mut blob = Blob::create(pool.clone(), secret_key.clone(), Locator::Root);
        blob.write(&content[..]).await.unwrap();
        blob.flush().await.unwrap();

        let mut read_buffer = [0];

        for &offset in &[0, 1, 2] {
            blob.seek(SeekFrom::Start(content.len() as u64 + offset))
                .await
                .unwrap();
            assert_eq!(blob.read(&mut read_buffer).await.unwrap(), 0);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn seek_before_start() {
        let (_, secret_key, pool) = setup(0).await;

        let content = b"content";

        let mut blob = Blob::create(pool.clone(), secret_key.clone(), Locator::Root);
        blob.write(&content[..]).await.unwrap();
        blob.flush().await.unwrap();

        let mut read_buffer = vec![0; content.len()];

        for &offset in &[0, 1, 2] {
            blob.seek(SeekFrom::End(-(content.len() as i64) - offset))
                .await
                .unwrap();
            blob.read(&mut read_buffer).await.unwrap();
            assert_eq!(read_buffer, content);
        }
    }

    // proptest doesn't work with the `#[tokio::test]` macro yet
    // (see https://github.com/AltSysrq/proptest/issues/179). As a workaround, create the runtime
    // manually.
    fn run<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_multi_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(future)
    }

    async fn setup(rng_seed: u64) -> (StdRng, SecretKey, db::Pool) {
        let mut rng = StdRng::seed_from_u64(rng_seed);
        let secret_key = SecretKey::generate(&mut rng);
        let pool = init_db().await;

        (rng, secret_key, pool)
    }

    fn rng_seed_strategy() -> impl Strategy<Value = u64> {
        any::<u64>().no_shrink()
    }

    async fn init_db() -> db::Pool {
        let pool = db::Pool::connect(":memory:").await.unwrap();
        index::init(&pool).await.unwrap();
        block::init(&pool).await.unwrap();
        pool
    }
}
