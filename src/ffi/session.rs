use super::{
    dart::{DartCObject, DartPort, PostCObjectFn},
    utils,
};
use std::{
    fmt,
    future::Future,
    io, mem,
    os::raw::{c_char, c_void},
    ptr,
};
use thiserror::Error;
use tokio::runtime::{self, Runtime};

/// Creates the ouisync session. `post_c_object_fn` should be a pointer to the dart's
/// `NativeApi.postCObject` function cast to `Pointer<Void>` (the casting is necessary to work
/// around limitations of the binding generators).
///
/// Returns `true` on success and `false` on error.
#[no_mangle]
pub unsafe extern "C" fn session_create(
    post_c_object_fn: *const c_void,
    error: *mut *const c_char,
) -> bool {
    if !SESSION.is_null() {
        try_ffi!(Err(SessionError::AlreadyInitialized), error);
    }

    // Init logger
    env_logger::init();

    let runtime = try_ffi!(
        runtime::Builder::new_multi_thread()
            .enable_time()
            .build()
            .map_err(SessionError::CreateRuntime),
        error
    );

    let session = Session {
        runtime,
        post_c_object_fn: mem::transmute(post_c_object_fn as *const ()),
    };

    SESSION = Box::into_raw(Box::new(session));

    true
}

/// Shutdowns the ouisync session.
#[no_mangle]
pub unsafe extern "C" fn session_destroy() {
    let session = mem::replace(&mut SESSION, ptr::null_mut());
    if !session.is_null() {
        let _ = Box::from_raw(session);
    }
}

/// Spawn a future on the session runtime and post the result over the given dart send port.
pub(super) unsafe fn spawn<F, T, E>(port: DartPort, error_ptr: *mut *const c_char, fut: F)
where
    F: Future<Output = Result<T, E>> + Send + 'static,
    T: Into<DartCObject>,
    E: fmt::Display,
{
    let session = if SESSION.is_null() {
        Err(SessionError::NotInitialized)
    } else {
        Ok(&*SESSION)
    };
    let session = try_ffi!(session, error_ptr);
    let post_cobject_fn = session.post_c_object_fn;

    let error_ptr = SendPtr(error_ptr);

    session.runtime.spawn(async move {
        match fut.await {
            Ok(value) => {
                utils::clear_error_ptr(error_ptr.0);
                post_cobject_fn(port, &mut value.into())
            }
            Err(error) => {
                utils::set_error_ptr(error_ptr.0, error);
                post_cobject_fn(port, &mut ().into())
            }
        }
    });
}

static mut SESSION: *mut Session = ptr::null_mut();

pub struct Session {
    runtime: Runtime,
    post_c_object_fn: PostCObjectFn,
}

#[derive(Debug, Error)]
enum SessionError {
    #[error("failed to create session runtime")]
    CreateRuntime(#[source] io::Error),
    #[error("session is not initialized")]
    NotInitialized,
    #[error("session is already initialized")]
    AlreadyInitialized,
}

// Wrapper that allows sending raw pointer across threads. Highly unsafe!
struct SendPtr<T>(*mut T);
unsafe impl<T> Send for SendPtr<T> {}
