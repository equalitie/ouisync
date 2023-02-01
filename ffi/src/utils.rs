use super::dart;
use ouisync_lib::{Error, Result};
use std::{
    ffi::{CStr, CString},
    marker::PhantomData,
    mem,
    os::raw::c_char,
    ptr,
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

/// FFI handle to a resource with unique ownership.
#[repr(transparent)]
pub struct UniqueHandle<T: 'static>(u64, PhantomData<&'static T>);

impl<T> UniqueHandle<T> {
    pub const NULL: Self = Self(0, PhantomData);

    pub(crate) fn new(resource: Box<T>) -> Self {
        Self(Box::into_raw(resource) as _, PhantomData)
    }

    pub(crate) unsafe fn get(&self) -> &T {
        assert_ne!(self.0, 0, "invalid handle");
        &*(self.0 as *const _)
    }

    pub(crate) unsafe fn release(self) -> Box<T> {
        Box::from_raw(self.0 as *mut _)
    }
}

pub(crate) unsafe fn ptr_to_str<'a>(ptr: *const c_char) -> Result<&'a str> {
    Ok(ptr_to_maybe_str(ptr)?.unwrap_or(""))
}

pub(crate) unsafe fn ptr_to_maybe_str<'a>(ptr: *const c_char) -> Result<Option<&'a str>> {
    if ptr.is_null() {
        return Ok(None);
    }

    Ok(Some(
        CStr::from_ptr(ptr)
            .to_str()
            .map_err(|_| Error::MalformedData)?,
    ))
}

pub(crate) fn str_to_c_string(s: &str) -> Result<CString> {
    CString::new(s.as_bytes()).map_err(|_| Error::MalformedData)
}

pub(crate) fn str_to_ptr(s: &str) -> *mut c_char {
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
