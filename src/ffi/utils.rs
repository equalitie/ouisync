use std::{
    ffi::{CStr, CString, OsStr},
    fmt,
    marker::PhantomData,
    mem,
    os::raw::c_char,
    ptr,
    sync::Arc,
};
use thiserror::Error;

use super::dart::DartCObject;

macro_rules! try_ffi {
    ($result:expr, $error_ptr:expr) => {
        match $result {
            Ok(value) => {
                super::utils::clear_error_ptr($error_ptr);
                value
            }
            Err(error) => {
                super::utils::set_error_ptr($error_ptr, error);
                return Default::default();
            }
        }
    };
}

pub unsafe fn set_error_ptr<E: fmt::Display>(error_ptr: *mut *const c_char, error: E) {
    if !error_ptr.is_null() {
        *error_ptr = CString::new(error.to_string()).unwrap().into_raw();
    }
}

pub unsafe fn clear_error_ptr(error_ptr: *mut *const c_char) {
    if !error_ptr.is_null() {
        *error_ptr = ptr::null();
    }
}

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

#[cfg(unix)]
pub fn c_str_to_os_str(c: &CStr) -> Result<&OsStr, EncodingError> {
    use std::os::unix::ffi::OsStrExt;
    Ok(OsStr::from_bytes(c.to_bytes()))
}

#[cfg(not(unix))]
pub fn c_str_to_os_str(c: &CStr) -> Result<&OsStr, EncodingError> {
    Ok(c.to_str().map_err(|_| EncodingError::InvalidUtf8)?.into())
}

pub fn os_str_to_c_string(os: &OsStr) -> Result<CString, EncodingError> {
    CString::new(os_str_as_bytes(os)?).map_err(|_| EncodingError::InteriorNul)
}

#[cfg(unix)]
fn os_str_as_bytes(os: &OsStr) -> Result<&[u8], EncodingError> {
    use std::os::unix::ffi::OsStrExt;
    Ok(os.as_bytes())
}

#[cfg(not(unix))]
fn os_str_to_bytes(os: &OsStr) -> Result<&[u8], EncodingError> {
    os.to_str().ok_or(EncodingError::InvalidUtf8).as_bytes()
}

#[derive(Debug, Error)]
pub enum EncodingError {
    #[cfg(not(unix))]
    #[error("string is not encoded as valid utf-8")]
    InvalidUtf8,
    #[error("string contains interior nul bytes")]
    InteriorNul,
}
