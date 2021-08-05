use crate::index::BranchData;
use camino::Utf8PathBuf;

/// Context needed for updating all necessary info when writing to a file or directory.
#[derive(Clone)]
pub struct WriteContext {
    path: Utf8PathBuf,
    local_branch: BranchData,
    // ancestors: Vec<Directory>,
}

impl WriteContext {
    pub fn new(path: Utf8PathBuf, local_branch: BranchData) -> Self {
        Self { path, local_branch }
    }

    pub fn child(&self, name: &str) -> Self {
        Self {
            path: self.path.join(name),
            local_branch: self.local_branch.clone(),
        }
    }

    pub fn local_branch(&self) -> &BranchData {
        &self.local_branch
    }
}
