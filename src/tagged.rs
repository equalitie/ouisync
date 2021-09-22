use std::{
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    ops::Deref,
};

/// Wrapper for arbitrary type that allows to attach additional compile-time checked semantics to
/// it.
#[repr(transparent)]
pub(crate) struct Tagged<V, T> {
    value: V,
    _tag: PhantomData<T>,
}

impl<V, T> Tagged<V, T> {
    pub fn new(value: V) -> Self {
        Self {
            value,
            _tag: PhantomData,
        }
    }

    pub fn into_inner(self) -> V {
        self.value
    }
}

impl<V, T> Deref for Tagged<V, T> {
    type Target = V;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<V, T> PartialEq for Tagged<V, T>
where
    V: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.value.eq(&other.value)
    }
}

impl<V, T> Eq for Tagged<V, T> where V: Eq {}

impl<V, T> Hash for Tagged<V, T>
where
    V: Hash,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state)
    }
}

impl<V, T> Clone for Tagged<V, T>
where
    V: Clone,
{
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            _tag: PhantomData,
        }
    }
}

impl<V, T> Copy for Tagged<V, T> where V: Copy {}

impl<V, T> fmt::Debug for Tagged<V, T>
where
    V: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.value.fmt(f)
    }
}

/// Wrapper for types with `Local` semantics.
pub(crate) type Local<V> = Tagged<V, LocalTag>;

/// Wrapper for types with `Remote` semantics.
pub(crate) type Remote<V> = Tagged<V, RemoteTag>;

pub(crate) struct LocalTag;
pub(crate) struct RemoteTag;
