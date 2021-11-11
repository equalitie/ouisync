use super::dart::{self, DartCObject};
use crate::{
    db,
    error::{self, Error, Result},
};
use camino::Utf8PathBuf;
use std::{
    ffi::{CStr, CString},
    marker::PhantomData,
    mem,
    os::raw::c_char,
    sync::Arc,
};

/// Type-safe wrapper over native dart SendPort.
#[repr(transparent)]
pub struct Port<T>(dart::Port, PhantomData<T>);

impl<T> From<Port<T>> for dart::Port {
    fn from(typed: Port<T>) -> Self {
        typed.0
    }
}

// `Port` is `Send`, `Copy` and `Clone` regardless of whether `T` is because it doesn't
// actually contain `T`:

unsafe impl<T> Send for Port<T> {}

impl<T> Clone for Port<T> {
    fn clone(&self) -> Self {
        Self(self.0, PhantomData)
    }
}

impl<T> Copy for Port<T> {}

/// FFI handle to a resource with shared ownership.
#[repr(transparent)]
pub struct SharedHandle<T>(u64, PhantomData<T>);

impl<T> SharedHandle<T> {
    pub fn new(resource: Arc<T>) -> Self {
        Self(Arc::into_raw(resource) as _, PhantomData)
    }

    pub unsafe fn get(self) -> Arc<T> {
        let res1 = Arc::from_raw(self.0 as *mut T);
        let res2 = res1.clone();
        mem::forget(res1);
        res2
    }

    pub unsafe fn release(self) -> Arc<T> {
        Arc::from_raw(self.0 as *mut _)
    }
}

impl<T> From<SharedHandle<T>> for DartCObject {
    fn from(handle: SharedHandle<T>) -> Self {
        DartCObject::from(handle.0)
    }
}

/// FFI handle to a resource with unique ownership.
#[repr(transparent)]
pub struct UniqueHandle<T>(u64, PhantomData<T>);

impl<T> UniqueHandle<T> {
    pub fn new(resource: Box<T>) -> Self {
        Self(Box::into_raw(resource) as _, PhantomData)
    }

    pub unsafe fn get(&self) -> &T {
        &*(self.0 as *const _)
    }

    pub unsafe fn release(self) -> Box<T> {
        Box::from_raw(self.0 as *mut _)
    }
}

impl<T> From<UniqueHandle<T>> for DartCObject {
    fn from(handle: UniqueHandle<T>) -> Self {
        DartCObject::from(handle.0)
    }
}

/// FFI handle to a borrowed resource.
#[repr(transparent)]
pub struct RefHandle<T>(u64, PhantomData<T>);

impl<T> RefHandle<T> {
    pub const NULL: Self = Self(0, PhantomData);

    pub fn new(resource: &T) -> Self {
        Self(resource as *const _ as _, PhantomData)
    }

    pub unsafe fn get(&self) -> &T {
        assert!(self.0 != 0);
        &*(self.0 as *const _)
    }
}

// Wrapper that bypasses the type-checker to allow sending non-Send types across threads.
// Highly unsafe!
#[derive(Clone, Copy)]
pub(super) struct AssumeSend<T>(T);

unsafe impl<T> Send for AssumeSend<T> {}

impl<T> AssumeSend<T> {
    pub unsafe fn new(value: T) -> Self {
        Self(value)
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

pub unsafe fn ptr_to_str<'a>(ptr: *const c_char) -> Result<&'a str> {
    CStr::from_ptr(ptr)
        .to_str()
        .map_err(|_| Error::MalformedData)
}

pub unsafe fn ptr_to_path_buf(ptr: *const c_char) -> Result<Utf8PathBuf> {
    Ok(Utf8PathBuf::from(ptr_to_str(ptr)?))
}

pub unsafe fn ptr_to_store(ptr: *const c_char) -> Result<db::Store> {
    Ok(error::into_ok(ptr_to_str(ptr)?.parse()))
}

pub fn str_to_c_string(s: &str) -> Result<CString> {
    CString::new(s.as_bytes()).map_err(|_| Error::MalformedData)
}
