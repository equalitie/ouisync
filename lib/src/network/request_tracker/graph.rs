use super::{MessageKey, PendingRequest, RequestVariant};
use crate::{
    collections::{HashMap, HashSet},
    network::message::Request,
};
use slab::Slab;
use std::collections::hash_map::Entry;

/// DAG for storing data for the request tracker.
pub(super) struct Graph<T> {
    index: HashMap<(MessageKey, RequestVariant), Key>,
    nodes: Slab<Node<T>>,
}

impl<T> Graph<T> {
    pub fn new() -> Self {
        Self {
            index: HashMap::default(),
            nodes: Slab::new(),
        }
    }

    pub fn get_or_insert(
        &mut self,
        request: PendingRequest,
        parent_key: Option<Key>,
        value: T,
    ) -> Key {
        let node_key = match self
            .index
            .entry((MessageKey::from(&request.payload), request.variant))
        {
            Entry::Occupied(entry) => {
                self.nodes
                    .get_mut(entry.get().0)
                    .expect("dangling index entry")
                    .parents
                    .extend(parent_key);

                *entry.get()
            }
            Entry::Vacant(entry) => {
                let node_key = self.nodes.insert(Node {
                    request,
                    parents: parent_key.into_iter().collect(),
                    children: HashSet::default(),
                    value,
                });
                let node_key = Key(node_key);

                entry.insert(node_key);

                node_key
            }
        };

        if let Some(parent_key) = parent_key {
            if let Some(parent_node) = self.nodes.get_mut(parent_key.0) {
                parent_node.children.insert(node_key);
            }
        }

        node_key
    }

    pub fn get(&self, key: Key) -> Option<&Node<T>> {
        self.nodes.get(key.0)
    }

    pub fn get_mut(&mut self, key: Key) -> Option<&mut Node<T>> {
        self.nodes.get_mut(key.0)
    }

    #[expect(unused)]
    pub fn lookup(&self, request_key: MessageKey, variant: RequestVariant) -> Option<&Node<T>> {
        self.index
            .get(&(request_key, variant))
            .and_then(|key| self.nodes.get(key.0))
    }

    pub fn lookup_mut(
        &mut self,
        request_key: MessageKey,
        variant: RequestVariant,
    ) -> Option<&mut Node<T>> {
        self.index
            .get(&(request_key, variant))
            .and_then(|key| self.nodes.get_mut(key.0))
    }

    pub fn remove(&mut self, key: Key) -> Option<Node<T>> {
        let node = self.nodes.try_remove(key.0)?;

        self.index.remove(&(
            MessageKey::from(&node.request.payload),
            node.request.variant,
        ));

        for parent_key in &node.parents {
            let Some(parent_node) = self.nodes.get_mut(parent_key.0) else {
                continue;
            };

            parent_node.children.remove(&key);
        }

        for child_key in &node.children {
            let Some(child_node) = self.nodes.get_mut(child_key.0) else {
                continue;
            };

            child_node.parents.remove(&key);
        }

        Some(node)
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub fn requests(&self) -> impl ExactSizeIterator<Item = &Request> {
        self.nodes.iter().map(|(_, node)| &node.request.payload)
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub(super) struct Key(usize);

pub(super) struct Node<T> {
    request: PendingRequest,
    parents: HashSet<Key>,
    children: HashSet<Key>,
    value: T,
}

impl<T> Node<T> {
    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn value_mut(&mut self) -> &mut T {
        &mut self.value
    }

    pub fn request(&self) -> &PendingRequest {
        &self.request
    }

    pub fn parents(&self) -> impl ExactSizeIterator<Item = Key> + '_ {
        self.parents.iter().copied()
    }

    pub fn children(&self) -> impl ExactSizeIterator<Item = Key> + '_ {
        self.children.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{network::debug_payload::DebugRequest, protocol::MultiBlockPresence};
    use rand::Rng;

    #[test]
    fn child_request() {
        let mut rng = rand::thread_rng();
        let mut graph = Graph::new();

        assert_eq!(graph.requests().len(), 0);

        let parent_request = Request::ChildNodes(rng.gen(), DebugRequest::start());

        let parent_node_key = graph.get_or_insert(
            PendingRequest {
                payload: parent_request.clone(),
                variant: RequestVariant::default(),
            },
            None,
            1,
        );

        assert_eq!(graph.requests().len(), 1);

        let Some(node) = graph.get(parent_node_key) else {
            unreachable!()
        };

        assert_eq!(*node.value(), 1);
        assert_eq!(node.children().len(), 0);
        assert_eq!(node.request().payload, parent_request);

        let child_request = Request::ChildNodes(rng.gen(), DebugRequest::start());

        let child_node_key = graph.get_or_insert(
            PendingRequest {
                payload: child_request.clone(),
                variant: RequestVariant::default(),
            },
            Some(parent_node_key),
            2,
        );

        assert_eq!(graph.requests().len(), 2);

        let Some(node) = graph.get(child_node_key) else {
            unreachable!()
        };

        assert_eq!(*node.value(), 2);
        assert_eq!(node.children().len(), 0);
        assert_eq!(node.request().payload, child_request);

        assert_eq!(
            graph
                .get(parent_node_key)
                .unwrap()
                .children()
                .collect::<Vec<_>>(),
            [child_node_key]
        );

        graph.remove(child_node_key);

        assert_eq!(graph.get(parent_node_key).unwrap().children().len(), 0);
    }

    #[test]
    fn duplicate_request() {
        let mut rng = rand::thread_rng();
        let mut graph = Graph::new();

        assert_eq!(graph.requests().len(), 0);

        let request = Request::ChildNodes(rng.gen(), DebugRequest::start());

        let node_key0 = graph.get_or_insert(
            PendingRequest {
                payload: request.clone(),
                variant: RequestVariant::default(),
            },
            None,
            1,
        );
        assert_eq!(graph.requests().len(), 1);

        let node_key1 = graph.get_or_insert(
            PendingRequest {
                payload: request,
                variant: RequestVariant::default(),
            },
            None,
            1,
        );
        assert_eq!(graph.requests().len(), 1);
        assert_eq!(node_key0, node_key1);
    }

    #[test]
    fn multiple_parents() {
        let mut rng = rand::thread_rng();
        let mut graph = Graph::new();

        let hash = rng.gen();

        let parent_request_0 = PendingRequest {
            payload: Request::ChildNodes(hash, DebugRequest::start()),
            variant: RequestVariant::new(MultiBlockPresence::None, MultiBlockPresence::None),
        };

        let parent_request_1 = PendingRequest {
            payload: Request::ChildNodes(hash, DebugRequest::start()),
            variant: RequestVariant::new(MultiBlockPresence::None, MultiBlockPresence::Full),
        };

        let child_request = Request::Block(rng.gen(), DebugRequest::start());

        let parent_key_0 = graph.get_or_insert(parent_request_0, None, 0);
        let parent_key_1 = graph.get_or_insert(parent_request_1, None, 1);

        let child_key_0 = graph.get_or_insert(
            PendingRequest {
                payload: child_request.clone(),
                variant: RequestVariant::default(),
            },
            Some(parent_key_0),
            2,
        );

        let child_key_1 = graph.get_or_insert(
            PendingRequest {
                payload: child_request,
                variant: RequestVariant::default(),
            },
            Some(parent_key_1),
            2,
        );

        assert_eq!(child_key_0, child_key_1);

        for parent_key in [parent_key_0, parent_key_1] {
            assert_eq!(
                graph
                    .get(parent_key)
                    .unwrap()
                    .children()
                    .collect::<HashSet<_>>(),
                HashSet::from([child_key_0])
            );
        }

        assert_eq!(
            graph
                .get(child_key_0)
                .unwrap()
                .parents()
                .collect::<HashSet<_>>(),
            HashSet::from([parent_key_0, parent_key_1])
        );

        graph.remove(parent_key_0);

        assert_eq!(
            graph
                .get(child_key_0)
                .unwrap()
                .parents()
                .collect::<HashSet<_>>(),
            HashSet::from([parent_key_1])
        );

        graph.remove(parent_key_1);

        assert_eq!(
            graph
                .get(child_key_0)
                .unwrap()
                .parents()
                .collect::<HashSet<_>>(),
            HashSet::default(),
        );
    }

    #[test]
    fn multiple_children() {
        let mut rng = rand::thread_rng();
        let mut graph = Graph::new();

        let parent_request = PendingRequest {
            payload: Request::ChildNodes(rng.gen(), DebugRequest::start()),
            variant: RequestVariant::default(),
        };

        let child_request_0 = PendingRequest {
            payload: Request::ChildNodes(rng.gen(), DebugRequest::start()),
            variant: RequestVariant::default(),
        };

        let child_request_1 = PendingRequest {
            payload: Request::ChildNodes(rng.gen(), DebugRequest::start()),
            variant: RequestVariant::default(),
        };

        let parent_key = graph.get_or_insert(parent_request, None, 0);
        let child_key_0 = graph.get_or_insert(child_request_0, Some(parent_key), 1);
        let child_key_1 = graph.get_or_insert(child_request_1, Some(parent_key), 2);

        assert_eq!(
            graph
                .get(parent_key)
                .unwrap()
                .children()
                .collect::<HashSet<_>>(),
            HashSet::from([child_key_0, child_key_1])
        );

        for child_key in [child_key_0, child_key_1] {
            assert_eq!(
                graph
                    .get(child_key)
                    .unwrap()
                    .parents()
                    .collect::<HashSet<_>>(),
                HashSet::from([parent_key])
            );
        }

        graph.remove(child_key_0);

        assert_eq!(
            graph
                .get(parent_key)
                .unwrap()
                .children()
                .collect::<HashSet<_>>(),
            HashSet::from([child_key_1])
        );

        graph.remove(child_key_1);

        assert_eq!(
            graph
                .get(parent_key)
                .unwrap()
                .children()
                .collect::<HashSet<_>>(),
            HashSet::default()
        );
    }
}
