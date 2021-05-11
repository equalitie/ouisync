use super::{
    dart::{DartCObject, DartPort, PostCObjectFn},
    utils,
};
use crate::{
    db,
    error::{Error, Result},
};
use std::{
    ffi::CStr,
    fmt,
    future::Future,
    mem,
    os::raw::{c_char, c_void},
    path::PathBuf,
    ptr,
};
use tokio::runtime::{self, Runtime};

/// Creates the ouisync session. `post_c_object_fn` should be a pointer to the dart's
/// `NativeApi.postCObject` function cast to `Pointer<Void>` (the casting is necessary to work
/// around limitations of the binding generators).
#[no_mangle]
pub unsafe extern "C" fn session_create(
    post_c_object_fn: *const c_void,
    store: *const c_char,
    port: DartPort,
    error: *mut *const c_char,
) {
    // Init logger
    env_logger::init();

    let runtime = try_ffi!(
        runtime::Builder::new_multi_thread()
            .enable_time()
            .build()
            .map_err(Error::CreateRuntime),
        error
    );

    let post_c_object_fn = mem::transmute(post_c_object_fn);

    let store = CStr::from_ptr(store);
    let store = if store.to_bytes() == b":memory:" {
        db::Store::Memory
    } else {
        db::Store::File(PathBuf::from(try_ffi!(
            utils::c_str_to_os_str(store),
            error
        )))
    };

    let error = SendPtr(error);

    let handle = runtime.handle().clone();
    handle.spawn(async move {
        let result = init(runtime, store, post_c_object_fn).await;
        dispatch(post_c_object_fn, port, error, result)
    });
}

/// Shutdowns the ouisync session.
#[no_mangle]
pub unsafe extern "C" fn session_destroy() {
    let session = mem::replace(&mut SESSION, ptr::null_mut());
    if !session.is_null() {
        let _ = Box::from_raw(session);
    }
}

// Spawn a future on the session runtime and post the result over the given dart send port.
pub(super) unsafe fn spawn<F, T>(port: DartPort, error_ptr: *mut *const c_char, fut: F)
where
    F: Future<Output = Result<T>> + Send + 'static,
    T: Into<DartCObject>,
{
    let session = try_ffi!(get(), error_ptr);
    let post_c_object_fn = session.post_c_object_fn;
    let error_ptr = SendPtr(error_ptr);

    session
        .runtime
        .spawn(async move { dispatch(post_c_object_fn, port, error_ptr, fut.await) });
}

// Fetch the database pool from the global session.
pub(super) unsafe fn pool<'a>() -> Result<&'a db::Pool> {
    Ok(&get()?.pool)
}

// Initialize the global session.
async unsafe fn init(
    runtime: Runtime,
    store: db::Store,
    post_c_object_fn: PostCObjectFn,
) -> Result<()> {
    if !SESSION.is_null() {
        return Err(Error::SessionAlreadyInitialized);
    }

    let session = Session {
        runtime,
        pool: db::init(store).await?,
        post_c_object_fn,
    };

    SESSION = Box::into_raw(Box::new(session));

    Ok(())
}

// Get reference to the global session.
unsafe fn get<'a>() -> Result<&'a Session> {
    if SESSION.is_null() {
        Err(Error::SessionNotInitialized)
    } else {
        Ok(&*SESSION)
    }
}

unsafe fn dispatch<T, E>(
    post_c_object_fn: PostCObjectFn,
    port: DartPort,
    error_ptr: SendPtr<*const c_char>,
    result: Result<T, E>,
) where
    T: Into<DartCObject>,
    E: fmt::Display,
{
    match result {
        Ok(value) => {
            utils::clear_error_ptr(error_ptr.0);
            post_c_object_fn(port, &mut value.into());
        }
        Err(error) => {
            utils::set_error_ptr(error_ptr.0, error);
            post_c_object_fn(port, &mut ().into());
        }
    }
}

static mut SESSION: *mut Session = ptr::null_mut();

pub struct Session {
    runtime: Runtime,
    pool: db::Pool,
    post_c_object_fn: PostCObjectFn,
}

// Wrapper that allows sending raw pointer across threads. Highly unsafe!
struct SendPtr<T>(*mut T);
unsafe impl<T> Send for SendPtr<T> {}
