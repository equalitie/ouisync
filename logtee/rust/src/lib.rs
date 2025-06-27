#[cfg(any(target_os = "linux", target_os = "windows"))]
#[path = "redirect.rs"]
mod inner;

#[cfg(target_os = "android")]
#[path = "android.rs"]
mod inner;

// TODO: ios and macos

#[cfg(test)]
mod tests;

use std::{
    ffi::{c_char, c_void, CStr},
    io,
    path::Path,
    ptr,
};

use file_rotate::{compression::Compression, suffix::AppendCount, ContentLimit, FileRotate};
use inner::Inner;

/// Utility to capture the log messages produced by the running program and write them to the
/// specified file.
pub struct Logtee {
    inner: Option<Inner>,
}

impl Logtee {
    pub fn start(path: impl AsRef<Path>, options: RotateOptions) -> io::Result<Self> {
        let file = FileRotate::new(
            path,
            AppendCount::new(options.max_files),
            ContentLimit::Bytes(options.max_file_size),
            Compression::None,
            None,
        );
        let inner = Inner::new(file)?;

        Ok(Self { inner: Some(inner) })
    }

    pub fn stop(mut self) -> io::Result<()> {
        self.inner
            .take()
            .map(|inner| inner.close())
            .unwrap_or(Ok(()))
    }
}

impl Drop for Logtee {
    fn drop(&mut self) {
        self.inner.take().map(|inner| inner.close());
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RotateOptions {
    /// Max size in bytes of the log file before is rotated.
    pub max_file_size: usize,
    /// Max number of rotated files (i.e. excluding the main file) to keep.
    pub max_files: usize,
}

impl Default for RotateOptions {
    fn default() -> Self {
        Self {
            max_file_size: 1024 * 1024,
            max_files: 1,
        }
    }
}

/// # Safety
///
/// - `path` must be safe to pass to [std::ffi::CStr::from_ptr].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn logtee_start(
    path: *const c_char,
    rotate_max_file_size: u64,
    rotate_max_files: u64,
) -> *mut c_void {
    // Safety: `path` is safe to pass to `CStr::from_ptr` as per this function's safety contract.
    let path = unsafe { CStr::from_ptr(path) };
    let path = match path.to_str() {
        Ok(path) => path,
        Err(_) => return ptr::null_mut(),
    };

    let options = RotateOptions {
        max_file_size: rotate_max_file_size as _,
        max_files: rotate_max_files as _,
    };

    let logtee = match Logtee::start(path, options) {
        Ok(logtee) => logtee,
        Err(_) => return ptr::null_mut(),
    };

    Box::into_raw(Box::new(logtee)) as _
}

/// # Safety
///
/// - `handle` must have been previously obtained from `logtee_start` and not already passed to
///   `logtee_close`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn logtee_stop(handle: *const c_void) {
    unsafe {
        let _ = Box::from_raw(handle as *mut Logtee);
    }
}
