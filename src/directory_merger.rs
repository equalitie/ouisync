use crate::{
    directory::{self, Directory, EntryRef},
    error::{Error, Result},
    file::File,
};
use futures_util::future;
use std::{cmp::Ordering, collections::VecDeque, iter};

/// Utility to recursively merge two versions of a directory.
pub(crate) struct DirectoryMerger {
    // queue of (loca, remote) directory pairs to merge.
    queue: VecDeque<(Directory, Directory)>,
    fork_files: Vec<File>,
    fork_directories: Vec<Directory>,
}

impl DirectoryMerger {
    pub fn new(local_root: Directory, remote_root: Directory) -> Self {
        Self {
            queue: iter::once((local_root, remote_root)).collect(),
            fork_files: Vec::new(),
            fork_directories: Vec::new(),
        }
    }

    pub async fn step(&mut self) -> Result<bool> {
        let (local_dir, remote_dir) = if let Some(pair) = self.queue.pop_back() {
            pair
        } else {
            return Ok(false);
        };

        self.scan(&local_dir, &remote_dir).await?;
        self.commit(&local_dir).await?;

        Ok(true)
    }

    async fn scan(&mut self, local: &Directory, remote: &Directory) -> Result<()> {
        let (local, remote) = future::join(local.read(), remote.read()).await;

        for remote_entry in remote.entries() {
            self.scan_entry(&local, remote_entry).await?;
        }

        Ok(())
    }

    async fn scan_entry(
        &mut self,
        local: &directory::Reader<'_>,
        remote_entry: EntryRef<'_>,
    ) -> Result<()> {
        let local_entries = match local.lookup(remote_entry.name()) {
            Ok(local_entries) => local_entries,
            Err(Error::EntryNotFound) => {
                self.schedule_fork(remote_entry).await?;
                return Ok(());
            }
            Err(error) => return Err(error),
        };

        let mut fork = false;

        for local_entry in local_entries {
            match local_entry
                .version_vector()
                .partial_cmp(remote_entry.version_vector())
            {
                Some(Ordering::Less) => {
                    fork = true;
                }
                Some(Ordering::Equal) => unreachable!(),
                Some(Ordering::Greater) => return Ok(()),
                None => {
                    fork = true;
                }
            }
        }

        if fork {
            self.schedule_fork(remote_entry).await?;
        }

        Ok(())
    }

    async fn schedule_fork(&mut self, entry: EntryRef<'_>) -> Result<()> {
        match entry {
            EntryRef::File(entry) => self.fork_files.push(entry.open().await?),
            EntryRef::Directory(entry) => self.fork_directories.push(entry.open().await?),
        }

        Ok(())
    }

    async fn commit(&mut self, local: &Directory) -> Result<()> {
        for mut file in self.fork_files.drain(..) {
            file.fork().await?;
        }

        for remote_directory in self.fork_directories.drain(..) {
            let local_directory = remote_directory.fork().await?;
            self.queue.push_front((local_directory, remote_directory));
        }

        local.flush().await?;

        Ok(())
    }
}
