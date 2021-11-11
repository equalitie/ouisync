use serde::{Deserialize, Serialize};

/// Type of filesystem entry.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Deserialize, Serialize)]
pub enum EntryType {
    File,
    Directory,
}
