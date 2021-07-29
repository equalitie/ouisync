//! Overhaul of `JoinDirectory` with improved API and functionality. Will eventually replace it.

use crate::{
    crypto::Cryptor, entry::EntryType, error::Result, index::Index, locator::Locator,
    replica_id::ReplicaId,
};

pub struct JointDirectory {
    index: Index,
    cryptor: Cryptor,
    locators: Vec<Locator>,
    // entries: BTreeMap<EntryKey, EntryData>,
}

impl JointDirectory {
    pub async fn open(_index: Index, _cryptor: Cryptor, _locators: Vec<Locator>) -> Result<Self> {
        // ...
        todo!()
    }

    pub fn entries(&self) -> impl Iterator<Item = EntryInfo> {
        // TODO
        std::iter::empty()
    }
}

pub struct EntryInfo<'a> {
    dummy: &'a i32,
}

impl<'a> EntryInfo<'a> {
    pub fn name(&self) -> &'a str {
        todo!()
    }

    pub fn entry_type(&self) -> EntryType {
        todo!()
    }

    pub fn branch_id(&self) -> &'a ReplicaId {
        todo!()
    }
}

// #[derive(Eq, PartialEq, Ord, PartialOrd)]
// struct EntryKey {
//     name: String,
//     branch_id: ReplicaId,
// }

// struct EntryData {
//     entry_type: EntryType,
//     locator: Locator,
// }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, directory::Directory, index::BranchData};
    use futures_util::future;
    use std::collections::HashMap;

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
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflict_independently_created_files() {
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

        let entries: HashMap<_, _> = root
            .entries()
            .map(|entry| (entry.branch_id(), entry.name()))
            .collect();
        assert_eq!(entries.len(), 2);

        assert_eq!(entries.get(branches[0].replica_id()), Some(&"file.txt"));
        assert_eq!(entries.get(branches[1].replica_id()), Some(&"file.txt"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflict_copied_files() {
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

        let root = JointDirectory::open(
            index,
            Cryptor::Null,
            vec![*root0.locator(), *root1.locator()],
        )
        .await
        .unwrap();

        let entries: HashMap<_, _> = root
            .entries()
            .map(|entry| (entry.branch_id(), entry.name()))
            .collect();
        assert_eq!(entries.len(), 2);

        assert_eq!(entries.get(branches[0].replica_id()), Some(&"file.txt"));
        assert_eq!(entries.get(branches[1].replica_id()), Some(&"file.txt"));
    }

    async fn setup(branch_count: usize) -> Index {
        let pool = db::init(db::Store::Memory).await.unwrap();
        let branches =
            future::try_join_all((0..branch_count).map(|_| BranchData::new(&pool, rand::random())))
                .await
                .unwrap();

        Index::load(pool, *branches[0].replica_id()).await.unwrap()
    }
}
