use super::{super::INNER_LAYER_COUNT, *};
use crate::{crypto::Hashable, db, replica_id::ReplicaId, test_utils};
use assert_matches::assert_matches;
use futures::TryStreamExt;
use rand::prelude::*;
use std::{collections::HashMap, mem};
use test_strategy::proptest;

#[tokio::test(flavor = "multi_thread")]
async fn create_new_root_node() {
    let pool = setup().await;

    let replica_id = rand::random();
    let hash = rand::random::<u64>().hash();

    let mut tx = pool.begin().await.unwrap();
    let (node0, changed) = RootNode::create(&mut tx, &replica_id, hash).await.unwrap();
    assert!(changed);
    assert_eq!(node0.hash, hash);

    let node1 = RootNode::load_latest_or_create(&mut tx, &replica_id)
        .await
        .unwrap();
    assert_eq!(node1, node0);

    let nodes: Vec<_> = RootNode::load_all(&mut tx, &replica_id, 2)
        .try_collect()
        .await
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0], node0);
}

#[tokio::test(flavor = "multi_thread")]
async fn create_existing_root_node() {
    let pool = setup().await;

    let replica_id = rand::random();
    let hash = rand::random::<u64>().hash();

    let mut tx = pool.begin().await.unwrap();
    let (node0, _) = RootNode::create(&mut tx, &replica_id, hash).await.unwrap();

    let (node1, changed) = RootNode::create(&mut tx, &replica_id, hash).await.unwrap();
    assert_eq!(node0, node1);
    assert!(!changed);

    let nodes: Vec<_> = RootNode::load_all(&mut tx, &replica_id, 2)
        .try_collect()
        .await
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0], node0);
}

#[tokio::test(flavor = "multi_thread")]
async fn create_new_inner_node() {
    let pool = setup().await;

    let parent = rand::random::<u64>().hash();
    let hash = rand::random::<u64>().hash();
    let bucket = rand::random();

    let mut tx = pool.begin().await.unwrap();

    let node = InnerNode::new(hash);
    assert!(node.save(&mut tx, &parent, bucket).await.unwrap());

    let nodes = InnerNode::load_children(&mut tx, &parent).await.unwrap();

    assert_eq!(nodes.get(bucket), Some(&node));

    assert!((0..bucket).all(|b| nodes.get(b).is_none()));
    assert!((bucket + 1..=u8::MAX).all(|b| nodes.get(b).is_none()));
}

#[tokio::test(flavor = "multi_thread")]
async fn create_existing_inner_node() {
    let pool = setup().await;

    let parent = rand::random::<u64>().hash();
    let hash = rand::random::<u64>().hash();
    let bucket = rand::random();

    let mut tx = pool.begin().await.unwrap();

    let node0 = InnerNode::new(hash);
    node0.save(&mut tx, &parent, bucket).await.unwrap();

    let node1 = InnerNode {
        hash,
        is_complete: false,
    };
    assert!(!node1.save(&mut tx, &parent, bucket).await.unwrap());

    let nodes = InnerNode::load_children(&mut tx, &parent).await.unwrap();

    assert_eq!(nodes.get(bucket), Some(&node0));
    assert!((0..bucket).all(|b| nodes.get(b).is_none()));
    assert!((bucket + 1..=u8::MAX).all(|b| nodes.get(b).is_none()));
}

#[tokio::test(flavor = "multi_thread")]
async fn attempt_to_create_conflicting_inner_node() {
    let pool = setup().await;

    let parent = rand::random::<u64>().hash();
    let bucket = rand::random();

    let hash0 = rand::random::<u64>().hash();
    let hash1 = loop {
        let hash = rand::random::<u64>().hash();
        if hash != hash0 {
            break hash;
        }
    };

    let mut tx = pool.begin().await.unwrap();

    let node0 = InnerNode::new(hash0);
    node0.save(&mut tx, &parent, bucket).await.unwrap();

    let node1 = InnerNode::new(hash1);
    assert_matches!(node1.save(&mut tx, &parent, bucket).await, Err(_)); // TODO: match concrete error type
}

#[tokio::test(flavor = "multi_thread")]
async fn update_inner_node_to_complete() {
    let pool = setup().await;
    let mut tx = pool.begin().await.unwrap();

    let parent = rand::random::<u64>().hash();
    let bucket = rand::random();
    let hash = rand::random::<u64>().hash();

    let mut node = InnerNode::new(hash);
    node.save(&mut tx, &parent, bucket).await.unwrap();

    let nodes = InnerNode::load_children(&mut tx, &parent).await.unwrap();
    assert!(!nodes.get(bucket).unwrap().is_complete);

    node.is_complete = true;
    node.save(&mut tx, &parent, bucket).await.unwrap();

    let nodes = InnerNode::load_children(&mut tx, &parent).await.unwrap();
    assert!(nodes.get(bucket).unwrap().is_complete);
}

#[proptest]
fn check_complete(
    #[strategy(0usize..=32)] leaf_count: usize,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(check_complete_case(leaf_count, rng_seed))
}

async fn check_complete_case(leaf_count: usize, rng_seed: u64) {
    let mut rng = StdRng::seed_from_u64(rng_seed);

    let pool = setup().await;
    let mut tx = pool.begin().await.unwrap();

    let snapshot = Snapshot::generate(&mut rng, leaf_count);

    let (mut root_node, _) = RootNode::create(&mut tx, &snapshot.replica_id, snapshot.root_hash)
        .await
        .unwrap();

    if leaf_count > 0 {
        assert!(!root_node.check_complete(&mut tx).await.unwrap());
    }

    // TODO: consider randomizing the order the nodes are saved so it's not always
    // breadth-first.

    for (inner_layer, maps) in snapshot.inners.iter().enumerate() {
        for (path, nodes) in maps {
            let parent_hash = snapshot.parent_hash(inner_layer, path);

            for (bucket, node) in nodes {
                node.save(&mut tx, parent_hash, bucket).await.unwrap();
            }

            assert!(!root_node.check_complete(&mut tx).await.unwrap());
        }
    }

    let mut unsaved_leaves: usize = snapshot.leaves.values().map(|nodes| nodes.len()).sum();

    for (path, nodes) in &snapshot.leaves {
        let parent_hash = snapshot.parent_hash(INNER_LAYER_COUNT, path);

        for node in nodes {
            node.save(&mut tx, parent_hash).await.unwrap();
            unsaved_leaves -= 1;
        }

        if unsaved_leaves > 0 {
            assert!(!root_node.check_complete(&mut tx).await.unwrap());
        }
    }

    assert!(root_node.check_complete(&mut tx).await.unwrap());
}

async fn setup() -> db::Pool {
    let pool = db::Pool::connect(":memory:").await.unwrap();
    super::super::init(&pool).await.unwrap();
    pool
}

// In-memory snapshot for testing purposes.
struct Snapshot {
    replica_id: ReplicaId,
    root_hash: Hash,
    inners: [HashMap<BucketPath, InnerNodeMap>; INNER_LAYER_COUNT],
    leaves: HashMap<BucketPath, LeafNodeSet>,
}

impl Snapshot {
    // Generate a random snapshot with the given maximum number of leaf nodes.
    fn generate<R: Rng>(rng: &mut R, leaf_count: usize) -> Self {
        let replica_id = rng.gen();
        let leaves = (0..leaf_count)
            .map(|_| {
                let locator = rng.gen::<u64>().hash();
                let block_id = rng.gen();
                LeafNode::new(locator, block_id)
            })
            .collect();

        Self::build(replica_id, leaves)
    }

    fn build(replica_id: ReplicaId, leaves: Vec<LeafNode>) -> Self {
        let leaves = leaves
            .into_iter()
            .fold(HashMap::<_, LeafNodeSet>::new(), |mut map, leaf| {
                map.entry(BucketPath::new(leaf.locator(), INNER_LAYER_COUNT - 1))
                    .or_default()
                    .modify(leaf.locator(), &leaf.block_id);
                map
            });

        let mut inners: [HashMap<_, InnerNodeMap>; INNER_LAYER_COUNT] = Default::default();

        for (path, set) in &leaves {
            add_inner_node(
                INNER_LAYER_COUNT - 1,
                &mut inners[INNER_LAYER_COUNT - 1],
                path,
                set.hash(),
            );
        }

        for layer in (0..INNER_LAYER_COUNT - 1).rev() {
            let (lo, hi) = inners.split_at_mut(layer + 1);

            for (path, map) in &hi[0] {
                add_inner_node(layer, lo.last_mut().unwrap(), path, map.hash());
            }
        }

        let root_hash = inners[0]
            .get(&BucketPath::default())
            .unwrap_or(&InnerNodeMap::default())
            .hash();

        Self {
            replica_id,
            root_hash,
            inners,
            leaves,
        }
    }

    // Returns the parent hash of inner nodes at `inner_layer` with the specified bucket path.
    fn parent_hash(&self, inner_layer: usize, path: &BucketPath) -> &Hash {
        if inner_layer == 0 {
            &self.root_hash
        } else {
            let (bucket, parent_path) = path.pop(inner_layer - 1);
            &self.inners[inner_layer - 1]
                .get(&parent_path)
                .unwrap()
                .get(bucket)
                .unwrap()
                .hash
        }
    }
}

fn add_inner_node(
    inner_layer: usize,
    maps: &mut HashMap<BucketPath, InnerNodeMap>,
    path: &BucketPath,
    hash: Hash,
) {
    let (bucket, parent_path) = path.pop(inner_layer);
    maps.entry(parent_path)
        .or_default()
        .insert(bucket, InnerNode::new(hash));
}

#[derive(Default, Clone, Copy, Eq, PartialEq, Hash, Debug)]
struct BucketPath([u8; INNER_LAYER_COUNT]);

impl BucketPath {
    fn new(locator: &Hash, inner_layer: usize) -> Self {
        let mut path = Self(Default::default());
        for (layer, bucket) in path.0.iter_mut().enumerate().take(inner_layer + 1) {
            *bucket = get_bucket(locator, layer)
        }
        path
    }

    fn pop(&self, inner_layer: usize) -> (u8, Self) {
        let mut popped = *self;
        let bucket = mem::replace(&mut popped.0[inner_layer], 0);
        (bucket, popped)
    }
}
