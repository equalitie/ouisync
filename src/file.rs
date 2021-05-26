use crate::blob::Blob;

// File is currently the same as `Blob` but this might change in the future.
// TODO: consider making it a newtype.
pub type File = Blob;
