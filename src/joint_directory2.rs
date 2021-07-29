//! Overhaul of `JoinDirectory` with improved API and functionality. Will eventually replace it.

use crate::{
    crypto::Cryptor,
    directory::Directory,
    entry::EntryType,
    error::{Error, Result},
    index::Index,
    locator::Locator,
    replica_id::ReplicaId,
};
use either::Either;
use std::{
    collections::{BTreeMap, HashMap},
    fmt, iter,
};

pub struct JointDirectory {
    index: Index,
    cryptor: Cryptor,
    entries: BTreeMap<EntryKey, EntryData>,
}

impl JointDirectory {
    pub async fn open(index: Index, cryptor: Cryptor, locators: Vec<Locator>) -> Result<Self> {
        let mut entries: BTreeMap<EntryKey, EntryData> = BTreeMap::new();

        for branch in index.branches().await {
            for locator in &locators {
                let dir = match Directory::open(
                    index.pool.clone(),
                    branch.clone(),
                    cryptor.clone(),
                    *locator,
                )
                .await
                {
                    Ok(dir) => dir,
                    Err(Error::EntryNotFound) => continue,
                    Err(error) => return Err(error),
                };

                for entry in dir.entries() {
                    let key = EntryKey {
                        name: entry.name().to_owned(),
                        entry_type: entry.entry_type(),
                    };

                    entries
                        .entry(key)
                        .or_default()
                        .insert(*branch.replica_id(), *locator);
                }
            }
        }

        Ok(Self {
            index,
            cryptor,
            entries,
        })
    }

    pub fn entries(&self) -> impl Iterator<Item = EntryInfo> {
        self.entries
            .iter()
            .map(|(key, data)| match key.entry_type {
                EntryType::File => {
                    Either::Left(data.iter().map(move |(branch_id, locator)| EntryInfo {
                        name: &key.name,
                        data: EntryDataView::File { branch_id, locator },
                    }))
                }
                EntryType::Directory => Either::Right(iter::once(EntryInfo {
                    name: &key.name,
                    data: EntryDataView::Directory(data),
                })),
            })
            .flatten()
    }

    pub fn lookup(&self, name: &str) -> Result<EntryInfo> {
        todo!()
    }
}

#[derive(Eq, PartialEq)]
pub struct EntryInfo<'a> {
    // index: &'a Index,
    name: &'a str,
    data: EntryDataView<'a>,
}

impl<'a> EntryInfo<'a> {
    pub fn name(&self) -> &'a str {
        self.name
    }

    pub fn entry_type(&self) -> EntryType {
        self.data.entry_type()
    }

    pub fn branch_id(&self) -> Option<&'a ReplicaId> {
        match &self.data {
            EntryDataView::File { branch_id, .. } => Some(branch_id),
            EntryDataView::Directory(_) => None,
        }
    }
}

impl<'a> fmt::Debug for EntryInfo<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EntryInfo")
            .field("name", &self.name)
            .field("type", &self.data.entry_type())
            .finish()
    }
}

#[derive(Eq, PartialEq, Ord, PartialOrd)]
struct EntryKey {
    name: String,
    entry_type: EntryType,
}

type EntryData = HashMap<ReplicaId, Locator>;

#[derive(Eq, PartialEq)]
enum EntryDataView<'a> {
    File {
        branch_id: &'a ReplicaId,
        locator: &'a Locator,
    },
    Directory(&'a EntryData),
}

impl<'a> EntryDataView<'a> {
    fn entry_type(&self) -> EntryType {
        match self {
            Self::File { .. } => EntryType::File,
            Self::Directory(_) => EntryType::Directory,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, directory::Directory, index::BranchData};
    use assert_matches::assert_matches;
    use futures_util::future;

    #[tokio::test(flavor = "multi_thread")]
    async fn no_conflict() {
        let index = setup(2).await;
        let branches = index.branches().await;

        let mut root0 = Directory::create(
            index.pool.clone(),
            branches[0].clone(),
            Cryptor::Null,
            Locator::Root,
        );
        root0
            .create_file("file0.txt".to_owned())
            .unwrap()
            .flush()
            .await
            .unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create(
            index.pool.clone(),
            branches[1].clone(),
            Cryptor::Null,
            Locator::Root,
        );
        root1
            .create_file("file1.txt".to_owned())
            .unwrap()
            .flush()
            .await
            .unwrap();
        root1.flush().await.unwrap();

        let root = JointDirectory::open(
            index,
            Cryptor::Null,
            vec![*root0.locator(), *root1.locator()],
        )
        .await
        .unwrap();

        let entries: Vec<_> = root.entries().collect();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name(), "file0.txt");
        assert_eq!(entries[0].entry_type(), EntryType::File);
        assert_eq!(entries[1].name(), "file1.txt");
        assert_eq!(entries[1].entry_type(), EntryType::File);

        // assert_eq!(root.lookup("file0.txt").unwrap(), entries[0]);
        // assert_eq!(root.lookup("file1.txt").unwrap(), entries[1]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflict_independent_files() {
        let index = setup(2).await;
        let branches = index.branches().await;

        let mut root0 = Directory::create(
            index.pool.clone(),
            branches[0].clone(),
            Cryptor::Null,
            Locator::Root,
        );
        root0
            .create_file("file.txt".to_owned())
            .unwrap()
            .flush()
            .await
            .unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create(
            index.pool.clone(),
            branches[1].clone(),
            Cryptor::Null,
            Locator::Root,
        );
        root1
            .create_file("file.txt".to_owned())
            .unwrap()
            .flush()
            .await
            .unwrap();
        root1.flush().await.unwrap();

        let root = JointDirectory::open(
            index,
            Cryptor::Null,
            vec![*root0.locator(), *root1.locator()],
        )
        .await
        .unwrap();

        let entries: Vec<_> = root.entries().collect();

        assert_eq!(entries.len(), 2);

        for branch in &branches {
            let entry = entries
                .iter()
                .find(|entry| entry.branch_id() == Some(branch.replica_id()))
                .unwrap();
            assert_eq!(entry.name(), "file.txt");
            assert_eq!(entry.entry_type(), EntryType::File);

            // assert_eq!(
            //     root.lookup(&format!("file.txt.{:8x}", branch.replica_id()))
            //         .unwrap(),
            //     *entry
            // );
        }

        // assert_matches!(root.lookup("file.txt"), Err(Error::AmbiguousEntry(branch_ids)) => {
        //     assert_eq!(branch_ids.len(), 2);
        //     assert!(branch_ids.contains(branches[0].replica_id()));
        //     assert!(branch_ids.contains(branches[1].replica_id()));
        // });
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflict_forked_files() {
        let index = setup(2).await;
        let branches = index.branches().await;

        let mut root0 = Directory::create(
            index.pool.clone(),
            branches[0].clone(),
            Cryptor::Null,
            Locator::Root,
        );
        let mut file0 = root0.create_file("file.txt".to_owned()).unwrap();
        file0.flush().await.unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create(
            index.pool.clone(),
            branches[1].clone(),
            Cryptor::Null,
            Locator::Root,
        );
        root1
            .copy_file("file.txt", file0.locators(), &branches[0])
            .await
            .unwrap();
        root1.flush().await.unwrap();

        let root = JointDirectory::open(
            index,
            Cryptor::Null,
            vec![*root0.locator(), *root1.locator()],
        )
        .await
        .unwrap();

        let entries: Vec<_> = root.entries().collect();

        assert_eq!(entries.len(), 2);

        for branch in &branches {
            let entry = entries
                .iter()
                .find(|entry| entry.branch_id() == Some(branch.replica_id()))
                .unwrap();
            assert_eq!(entry.name(), "file.txt");
            assert_eq!(entry.entry_type(), EntryType::File);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflict_directories() {
        let index = setup(2).await;
        let branches = index.branches().await;

        let mut root0 = Directory::create(
            index.pool.clone(),
            branches[0].clone(),
            Cryptor::Null,
            Locator::Root,
        );

        let mut dir0 = root0.create_directory("dir".to_owned()).unwrap();
        dir0.flush().await.unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create(
            index.pool.clone(),
            branches[1].clone(),
            Cryptor::Null,
            Locator::Root,
        );

        let mut dir1 = root1.create_directory("dir".to_owned()).unwrap();
        dir1.flush().await.unwrap();
        root1.flush().await.unwrap();

        let root = JointDirectory::open(
            index,
            Cryptor::Null,
            vec![*root0.locator(), *root1.locator()],
        )
        .await
        .unwrap();

        let entries: Vec<_> = root.entries().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name(), "dir");
        assert_eq!(entries[0].entry_type(), EntryType::Directory);

        // TODO: verify locators
    }

    // TODO: test conflict_forked_directories
    // TODO: test conflict_file_and_directory

    async fn setup(branch_count: usize) -> Index {
        let pool = db::init(db::Store::Memory).await.unwrap();
        let branches =
            future::try_join_all((0..branch_count).map(|_| BranchData::new(&pool, rand::random())))
                .await
                .unwrap();

        Index::load(pool, *branches[0].replica_id()).await.unwrap()
    }
}
