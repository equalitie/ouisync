use bytes::Bytes;

/// Trait for asynchronously sending responses to the guest language.
pub(crate) trait Sender: Unpin + Send + 'static {
    fn send(&self, msg: Bytes);
}
