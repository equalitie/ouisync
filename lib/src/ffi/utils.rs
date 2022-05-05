use super::dart::{self, DartCObject};
use crate::error::{Error, Result};
use camino::Utf8PathBuf;
use std::path::PathBuf;
use std::{
    ffi::{CStr, CString},
    marker::PhantomData,
    mem,
    os::raw::c_char,
    ptr,
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
pub struct SharedHandle<T>(u64, PhantomData<*const T>);

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
pub struct UniqueHandle<T>(u64, PhantomData<*const T>);

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

/// FFI handle to a resource with unique ownership that can also be null.
#[repr(transparent)]
pub struct UniqueNullableHandle<T>(u64, PhantomData<*const T>);

impl<T> UniqueNullableHandle<T> {
    pub const NULL: Self = Self(0, PhantomData);

    pub fn new(resource: Box<T>) -> Self {
        Self(Box::into_raw(resource) as _, PhantomData)
    }

    pub unsafe fn get(&self) -> Option<&T> {
        if self.0 != 0 {
            Some(&*(self.0 as *const _))
        } else {
            None
        }
    }

    pub unsafe fn release(self) -> Option<Box<T>> {
        if self.0 != 0 {
            Some(Box::from_raw(self.0 as *mut _))
        } else {
            None
        }
    }
}

impl<T> From<UniqueNullableHandle<T>> for DartCObject {
    fn from(handle: UniqueNullableHandle<T>) -> Self {
        DartCObject::from(handle.0)
    }
}

/// FFI handle to a borrowed resource.
#[repr(transparent)]
pub struct RefHandle<T>(u64, PhantomData<*const T>);

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

#[allow(clippy::non_send_fields_in_send_ty)]
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
    if ptr.is_null() {
        return Ok("");
    }

    CStr::from_ptr(ptr)
        .to_str()
        .map_err(|_| Error::MalformedData)
}

pub unsafe fn ptr_to_path_buf(ptr: *const c_char) -> Result<Utf8PathBuf> {
    Ok(Utf8PathBuf::from(ptr_to_str(ptr)?))
}

pub unsafe fn ptr_to_native_path_buf(ptr: *const c_char) -> Result<PathBuf> {
    Ok(PathBuf::from(ptr_to_str(ptr)?))
}

pub fn str_to_c_string(s: &str) -> Result<CString> {
    CString::new(s.as_bytes()).map_err(|_| Error::MalformedData)
}

pub fn str_to_ptr(s: &str) -> *mut c_char {
    str_to_c_string(s)
        .map(CString::into_raw)
        .unwrap_or(ptr::null_mut())
}

#[repr(C)]
pub struct Bytes {
    pub ptr: *mut u8,
    pub len: u64,
}

impl Bytes {
    pub const NULL: Self = Self {
        ptr: ptr::null_mut(),
        len: 0,
    };

    pub fn from_vec(vec: Vec<u8>) -> Self {
        let mut slice = vec.into_boxed_slice();
        let buffer = Self {
            ptr: slice.as_mut_ptr(),
            len: slice.len() as u64,
        };
        mem::forget(slice);
        buffer
    }

    pub unsafe fn into_vec(self) -> Vec<u8> {
        Vec::from_raw_parts(self.ptr, self.len as usize, self.len as usize)
    }
}
