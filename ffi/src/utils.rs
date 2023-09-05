use std::{
    ffi::{CStr, CString},
    marker::PhantomData,
    os::raw::c_char,
    ptr,
    str::Utf8Error,
};

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

pub(crate) unsafe fn ptr_to_str<'a>(ptr: *const c_char) -> Result<&'a str, Utf8Error> {
    Ok(ptr_to_maybe_str(ptr)?.unwrap_or(""))
}

pub(crate) unsafe fn ptr_to_maybe_str<'a>(
    ptr: *const c_char,
) -> Result<Option<&'a str>, Utf8Error> {
    if ptr.is_null() {
        return Ok(None);
    }

    CStr::from_ptr(ptr).to_str().map(Some)
}

pub(crate) fn str_to_ptr(s: &str) -> *mut c_char {
    CString::new(s.as_bytes())
        .map(CString::into_raw)
        .unwrap_or(ptr::null_mut())
}
