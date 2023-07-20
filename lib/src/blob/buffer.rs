use crate::protocol::BLOCK_SIZE;
use std::{
    convert::TryInto,
    ops::{Deref, DerefMut},
};
use zeroize::Zeroize;

// Buffer for keeping loaded block content and also for in-place encryption and decryption.
#[derive(Clone)]
pub(super) struct Buffer(Box<[u8]>);

impl Buffer {
    pub fn new() -> Self {
        Self::default()
    }

    // Read data from `offset` of the buffer into a fixed-length array.
    //
    // # Panics
    //
    // Panics if the remaining length after `offset` is less than `N`.
    pub fn read_array<const N: usize>(&self, offset: usize) -> [u8; N] {
        self[offset..offset + N].try_into().unwrap()
    }

    // Read data from `offset` of the buffer into a `u64`.
    //
    // # Panics
    //
    // Panics if the remaining length is less than `size_of::<u64>()`
    pub fn read_u64(&self, offset: usize) -> u64 {
        u64::from_le_bytes(self.read_array(offset))
    }

    // Read data from offset into `dst`.
    pub fn read(&self, offset: usize, dst: &mut [u8]) {
        dst.copy_from_slice(&self.0[offset..offset + dst.len()]);
    }

    // Write a `u64` at `offset` into the buffer.
    pub fn write_u64(&mut self, offset: usize, value: u64) {
        let bytes = value.to_le_bytes();
        self.write(offset, &bytes[..]);
    }

    // Writes data from `dst` into the buffer.
    pub fn write(&mut self, offset: usize, src: &[u8]) {
        self.0[offset..offset + src.len()].copy_from_slice(src);
    }
}

impl Default for Buffer {
    fn default() -> Self {
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
