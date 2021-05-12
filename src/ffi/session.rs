use super::{
    dart::{DartCObject, DartPort, PostCObjectFn},
    utils,
};
use crate::{db, error::Result};
use std::{
    ffi::{CStr, CString},
    fmt,
    future::Future,
    mem,
    os::raw::{c_char, c_void},
    path::PathBuf,
    ptr,
};
use tokio::runtime::{self, Runtime};

/// Opens the ouisync session. `post_c_object_fn` should be a pointer to the dart's
/// `NativeApi.postCObject` function cast to `Pointer<Void>` (the casting is necessary to work
/// around limitations of the binding generators).
#[no_mangle]
pub unsafe extern "C" fn session_open(
    post_c_object_fn: *const c_void,
    store: *const c_char,
    port: DartPort,
    error_ptr: *mut *mut c_char,
) {
    let sender = Sender {
        post_c_object_fn: mem::transmute(post_c_object_fn),
    };

    if !SESSION.is_null() {
        sender.send_err(port, error_ptr, "session is already initialized");
        return;
    }

    // Init logger
    env_logger::init();

    let runtime = match runtime::Builder::new_multi_thread().enable_time().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            sender.send_err(port, error_ptr, error);
            return;
        }
    };

    let store = match store_from_raw(store) {
        Ok(store) => store,
        Err(error) => {
            sender.send_err(port, error_ptr, error);
            return;
        }
    };

    let handle = runtime.handle().clone();

    handle.spawn(sender.invoke(port, error_ptr, async move {
        let session = Session {
            runtime,
            pool: db::init(store).await?,
            sender,
        };

        assert!(SESSION.is_null());

        SESSION = Box::into_raw(Box::new(session));

        Ok(())
    }));
}

/// Closes the ouisync session.
#[no_mangle]
pub unsafe extern "C" fn session_close() {
    let session = mem::replace(&mut SESSION, ptr::null_mut());
    if !session.is_null() {
        let _ = Box::from_raw(session);
    }
}

unsafe fn store_from_raw(store: *const c_char) -> Result<db::Store> {
    let store = CStr::from_ptr(store);

    if store.to_bytes() == b":memory:" {
        Ok(db::Store::Memory)
    } else {
        Ok(db::Store::File(PathBuf::from(utils::c_str_to_os_str(
            store,
        )?)))
    }
}

pub(super) unsafe fn with<F>(port: DartPort, error_ptr: *mut *mut c_char, f: F)
where
    F: FnOnce(Context) -> Result<()>,
{
    assert!(!SESSION.is_null(), "session is not initialized");

    let session = &*SESSION;
    let context = Context {
        session,
        port,
        error_ptr,
    };

    match f(context) {
        Ok(()) => (),
        Err(error) => session.sender.send_err(port, error_ptr, error),
    }
}

static mut SESSION: *mut Session = ptr::null_mut();

struct Session {
    runtime: Runtime,
    pool: db::Pool,
    sender: Sender,
}

pub(super) struct Context<'a> {
    session: &'a Session,
    port: DartPort,
    error_ptr: *mut *mut c_char,
}

impl Context<'_> {
    pub(super) unsafe fn spawn<F, T>(self, f: F) -> Result<()>
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Into<DartCObject> + 'static,
    {
        self.session
            .runtime
            .spawn(self.session.sender.invoke(self.port, self.error_ptr, f));
        Ok(())
    }

    pub(super) fn pool(&self) -> &db::Pool {
        &self.session.pool
    }
}

// Utility for sending values to dart.
#[derive(Copy, Clone)]
struct Sender {
    post_c_object_fn: PostCObjectFn,
}

impl Sender {
    // NOTE: using explicit `impl Future` return instead of `async` to be able to specify the
    // returned future is `Send` even though `error_ptr` is not `Send`.
    unsafe fn invoke<F, T>(
        &self,
        port: DartPort,
        error_ptr: *mut *mut c_char,
        f: F,
    ) -> impl Future<Output = ()> + Send
    where
        F: Future<Output = Result<T>> + Send,
        T: Into<DartCObject>,
    {
        let error_ptr = AssumeSend(error_ptr);
        let sender = *self;

        async move {
            match f.await {
                Ok(value) => sender.send_ok(port, error_ptr.0, value),
                Err(error) => sender.send_err(port, error_ptr.0, error),
            }
        }
    }

    unsafe fn send_ok<T>(&self, port: DartPort, error_ptr: *mut *mut c_char, value: T)
    where
        T: Into<DartCObject>,
    {
        if !error_ptr.is_null() {
            *error_ptr = ptr::null_mut();
        }

        (self.post_c_object_fn)(port, &mut value.into());
    }

    unsafe fn send_err<E>(&self, port: DartPort, error_ptr: *mut *mut c_char, error: E)
    where
        E: fmt::Display,
    {
        if !error_ptr.is_null() {
            *error_ptr = CString::new(error.to_string()).unwrap().into_raw();
        }

        (self.post_c_object_fn)(port, &mut ().into());
    }
}

// Wrapper that bypasses the type-checker to allow sending non-Send types across threads.
// Highly unsafe!
struct AssumeSend<T>(T);
unsafe impl<T> Send for AssumeSend<T> {}
