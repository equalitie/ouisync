use super::MessageKey;
use crate::{collections::HashSet, network::message::Request, protocol::MultiBlockPresence};
use std::marker::PhantomData;

/// DAG for storing data for the request tracker.
pub(super) struct Graph<T> {
    _todo: PhantomData<T>,
}

impl<T> Graph<T> {
    pub fn new() -> Self {
        todo!()
    }

    pub fn entry(&mut self, _key: Key) -> Entry<'_, T> {
        todo!()
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub fn len(&self) -> usize {
        todo!()
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub(super) struct Key(pub MessageKey, pub MultiBlockPresence);

pub(super) enum Entry<'a, T> {
    #[expect(dead_code)]
    Occupied(OccupiedEntry<'a, T>),
    #[expect(dead_code)]
    Vacant(VacantEntry<'a, T>),
}

pub(super) struct OccupiedEntry<'a, T> {
    _todo: PhantomData<&'a mut T>,
}

impl<'a, T> OccupiedEntry<'a, T> {
    #[expect(dead_code)]
    pub fn get(&self) -> &T {
        todo!()
    }

    pub fn get_mut(&mut self) -> &mut T {
        todo!()
    }

    pub fn into_mut(self) -> &'a mut T {
        todo!()
    }

    pub fn request(&self) -> &'a Request {
        todo!()
    }

    pub fn parents(&self) -> &HashSet<Key> {
        todo!()
    }

    pub fn insert_parent(&mut self, _parent: Key) {
        todo!()
    }

    pub fn children(&self) -> &HashSet<Key> {
        todo!();
    }

    pub fn insert_child(&mut self, _child: Key) {
        todo!()
    }

    pub fn remove_child(&mut self, _child: &Key) {
        todo!()
    }

    pub fn remove(self) -> RemovedEntry<T> {
        todo!()
    }
}

pub(super) struct VacantEntry<'a, T> {
    _todo: PhantomData<&'a mut T>,
}

impl<'a, T> VacantEntry<'a, T> {
    pub fn insert(
        self,
        _request: Request,
        _parent: Option<Key>,
        _value: T,
    ) -> (&'a Request, &'a mut T) {
        todo!()
    }
}

pub(super) struct RemovedEntry<T> {
    #[expect(dead_code)]
    pub value: T,
    pub parents: HashSet<Key>,
}
