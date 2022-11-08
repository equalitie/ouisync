use crate::{
    crypto::Hash,
    repository::RepositoryId,
};

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

#[cfg(test)]
impl From<Content> for Request {
    fn from(content: Content) -> Self {
        match content {
            Content::Request(request) => request,
            Content::Response(_) | Content::Pex(_) => {
                panic!("not a request: {:?}", content)
            }
        }
    }
}

#[cfg(test)]
impl From<Content> for Response {
    fn from(content: Content) -> Self {
        match content {
            Content::Response(response) => response,
            Content::Request(_) | Content::Pex(_) => {
                panic!("not a response: {:?}", content)
            }
        }
    }
}
