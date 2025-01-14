use serde::{Deserialize, Serialize};

/// Edit of a single metadata entry.
#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct MetadataEdit {
    /// The key of the entry.
    pub key: String,
    /// The current value of the entry or `None` if the entry does not exist yet. This is used for
    /// concurrency control - if the current value is different from this it's assumed it has been
    /// modified by some other task and the whole `RepositorySetMetadata` operation is rolled back.
    /// If that happens, the user should read the current value again, adjust the new value if
    /// needed and retry the operation.
    pub old: Option<String>,
    /// The value to set the entry to or `None` to remove the entry.
    pub new: Option<String>,
}

// TODO: support more types than just string

// /// Metadata value
// #[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
// #[serde(untagged)]
// pub enum MetadataValue {
//     Bool(bool),
//     String(String),
// }

// impl From<bool> for MetadataValue {
//     fn from(value: bool) -> Self {
//         Self::Bool(value)
//     }
// }

// impl From<String> for MetadataValue {
//     fn from(value: String) -> Self {
//         Self::String(value)
//     }
// }
