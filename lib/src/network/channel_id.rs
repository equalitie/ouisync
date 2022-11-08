use crate::{crypto::Hash, repository::RepositoryId};

define_byte_array_wrapper! {
    // TODO: consider lower size (truncate the hash) which should still be enough to be unique
    // while reducing the message size.
    pub(crate) struct ChannelId([u8; Hash::SIZE]);
}

impl ChannelId {
    #[cfg(test)]
    pub(crate) fn random() -> Self {
        Self(rand::random())
    }
}

impl<'a> From<&'a RepositoryId> for ChannelId {
    fn from(id: &'a RepositoryId) -> Self {
        Self(id.salted_hash(b"ouisync message channel").into())
    }
}

impl Default for ChannelId {
    fn default() -> Self {
        Self([0; Self::SIZE])
    }
}
