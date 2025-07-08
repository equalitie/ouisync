use std::io;

use file_rotate::{suffix::AppendCount, FileRotate};

pub(super) struct Inner;

impl Inner {
    pub fn new(_file: FileRotate<AppendCount>) -> io::Result<Self> {
        Ok(Self)
    }

    pub fn close(self) -> io::Result<()> {
        Ok(())
    }
}
