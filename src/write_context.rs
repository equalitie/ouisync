use crate::branch::Branch;
use camino::Utf8PathBuf;

/// Context needed for updating all necessary info when writing to a file or directory.
#[derive(Clone)]
pub struct WriteContext {
    path: Utf8PathBuf,
    local_branch: Branch,
    // ancestors: Vec<Directory>,
}

impl WriteContext {
    pub fn new(path: Utf8PathBuf, local_branch: Branch) -> Self {
        Self { path, local_branch }
    }

    pub fn child(&self, name: &str) -> Self {
        Self {
            path: self.path.join(name),
            local_branch: self.local_branch.clone(),
        }
    }

    pub fn local_branch(&self) -> &Branch {
        &self.local_branch
    }
}
