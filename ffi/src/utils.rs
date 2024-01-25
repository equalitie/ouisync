use std::{
    ffi::{CStr, CString},
    os::raw::c_char,
    ptr,
    str::Utf8Error,
};

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
