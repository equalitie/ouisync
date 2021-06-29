use crate::{
    crypto::Cryptor,
    db,
    directory::Directory,
    error::Result,
    index::Branch,
    iterator::{accumulate::Accumulate, sorted_union},
    locator::Locator,
    replica_id::ReplicaId,
};

use std::{collections::BTreeMap, ffi::OsString};

struct JointDirectory {
    // Note: we may have a single directory in multiple branches, but we may also have multiple
    // versions of a single directory in any one branch.
    dirs: BTreeMap<ReplicaId, Directory>,
}

impl JointDirectory {
    pub async fn open_root<'a, I>(branches: I, pool: db::Pool, cryptor: Cryptor) -> Result<Self>
    where
        I: Iterator<Item = &'a Branch>,
    {
        let mut dirs = BTreeMap::<ReplicaId, Directory>::new();

        for branch in branches {
            let root =
                Directory::open(pool.clone(), branch.clone(), cryptor.clone(), Locator::Root)
                    .await?;
            dirs.insert(*branch.replica_id(), root);
        }

        Ok(Self { dirs })
    }

    pub fn entries(&self) -> impl Iterator<Item = OsString> + '_ {
        // Map<ReplicaId, Directory> -> [[(ReplicaId, EntryInfo)]]
        let entries = self.dirs.iter().map(|(replica_id, directory)| {
            let r_id = replica_id;
            directory.entries().map(move |e| (r_id, e))
        });

        // [[(ReplicaId, EntryInfo)]] -> [(ReplicaId, EntryInfo)]
        let flat_entries =
            sorted_union::new_from_many(entries, |(_replica_id, entry)| entry.name());

        // [(ReplicaId, EntryInfo)] -> [(entry-name, [(ReplicaId, EntryInfo)]]
        let grouped_entries = Accumulate::new(flat_entries, |(_replica_id, entry)| entry.name());

        grouped_entries.flat_map(|(name, entries)| {
            if entries.len() == 1 {
                vec![name.to_os_string()]
            } else {
                entries
                    .iter()
                    .map(|(replica_id, entry)| entry.name_with_label(replica_id))
                    .collect()
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_no_conflict() {
        let (pool, branches) = setup(2).await;

        let mut r0 = Directory::create(
            pool.clone(),
            branches[0].clone(),
            Cryptor::Null,
            Locator::Root,
        );
        let mut r1 = Directory::create(
            pool.clone(),
            branches[1].clone(),
            Cryptor::Null,
            Locator::Root,
        );

        r0.create_file("file0.txt".into())
            .unwrap()
            .flush()
            .await
            .unwrap();
        r1.create_file("file1.txt".into())
            .unwrap()
            .flush()
            .await
            .unwrap();

        r0.flush().await.unwrap();
        r1.flush().await.unwrap();

        let root = JointDirectory::open_root(branches.iter(), pool.clone(), Cryptor::Null)
            .await
            .unwrap();

        assert_eq!(
            root.entries().collect::<Vec<_>>(),
            vec!["file0.txt", "file1.txt"]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_conflict() {
        let (pool, branches) = setup(2).await;

        let mut r0 = Directory::create(
            pool.clone(),
            branches[0].clone(),
            Cryptor::Null,
            Locator::Root,
        );
        let mut r1 = Directory::create(
            pool.clone(),
            branches[1].clone(),
            Cryptor::Null,
            Locator::Root,
        );

        r0.create_file("file.txt".into())
            .unwrap()
            .flush()
            .await
            .unwrap();
        r1.create_file("file.txt".into())
            .unwrap()
            .flush()
            .await
            .unwrap();

        r0.flush().await.unwrap();
        r1.flush().await.unwrap();

        let root = JointDirectory::open_root(branches.iter(), pool.clone(), Cryptor::Null)
            .await
            .unwrap();

        assert_eq!(root.entries().count(), 2);
    }

    async fn setup(branch_count: usize) -> (db::Pool, Vec<Branch>) {
        let pool = db::init(db::Store::Memory).await.unwrap();

        let mut branches = Vec::new();

        for _ in 0..branch_count {
            let branch = Branch::new(&pool, ReplicaId::random()).await.unwrap();
            branches.push(branch);
        }

        (pool, branches)
    }
}
