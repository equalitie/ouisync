use crate::index::BranchData;
use camino::Utf8PathBuf;

/// Context needed for updating all necessary info when writing to a file or directory.
#[derive(Clone)]
pub struct WriteContext {
    pub path: Utf8PathBuf,
    pub local_branch: BranchData,
}

impl WriteContext {
    pub fn child(&self, name: &str) -> Self {
        Self {
            path: self.path.join(name),
            local_branch: self.local_branch.clone(),
        }
    }
}
