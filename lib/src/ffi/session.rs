use super::{
    dart::{DartCObject, PostDartCObjectFn},
    logger::Logger,
    utils::{self, AssumeSend, Port, SharedHandle},
};
use crate::{
    config, db,
    error::Result,
    network::{self, Network, NetworkOptions},
    this_replica,
};
use std::{
    ffi::CString,
    fmt,
    future::Future,
    mem,
    os::raw::{c_char, c_void},
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
    port: Port<()>,
    error_ptr: *mut *mut c_char,
) {
    let sender = Sender {
        post_c_object_fn: mem::transmute(post_c_object_fn),
    };

    if !SESSION.is_null() {
        // Session already exists.
        sender.send_ok(port, error_ptr, ());
        return;
    }

    // Init logger
    let logger = match Logger::new() {
        Ok(logger) => logger,
        Err(error) => {
            sender.send_err(port, error_ptr, error);
            return;
        }
    };

    let runtime = match runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            sender.send_err(port, error_ptr, error);
            return;
        }
    };

    let store = match utils::ptr_to_store(store) {
        Ok(store) => store,
        Err(error) => {
            sender.send_err(port, error_ptr, error);
            return;
        }
    };

    let handle = runtime.handle().clone();

    handle.spawn(sender.invoke(port, error_ptr, async move {
        let session = Session::new(runtime, store, sender, logger).await?;

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
        Box::from_raw(session);
    }
}

pub(super) unsafe fn with<T, F>(port: Port<T>, error_ptr: *mut *mut c_char, f: F)
where
    F: FnOnce(Context<T>) -> Result<()>,
{
    assert!(!SESSION.is_null(), "session is not initialized");

    let session = &*SESSION;
    let context = Context {
        session,
        port,
        error_ptr,
    };

    let _runtime_guard = context.session.runtime.enter();

    match f(context) {
        Ok(()) => (),
        Err(error) => session.sender.send_err(port, error_ptr, error),
    }
}

pub(super) unsafe fn get<'a>() -> &'a Session {
    assert!(!SESSION.is_null(), "session is not initialized");
    &*SESSION
}

static mut SESSION: *mut Session = ptr::null_mut();

pub(super) struct Session {
    runtime: Runtime,
    network: Network,
    sender: Sender,
    _logger: Logger,
}

impl Session {
    async fn new(
        runtime: Runtime,
        store: db::Store,
        sender: Sender,
        logger: Logger,
    ) -> Result<Self> {
        let pool = config::open_db(&store).await?;
        let this_replica_id = this_replica::get_or_create_id(&pool).await?;
        let network = Network::new(this_replica_id, &NetworkOptions::default()).await?;

        Ok(Self {
            runtime,
            network,
            sender,
            _logger: logger,
        })
    }

    pub(super) fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub(super) fn sender(&self) -> Sender {
        self.sender
    }

    pub(super) fn network(&self) -> &Network {
        &self.network
    }
}

#[no_mangle]
pub unsafe extern "C" fn session_get_network(
    port: Port<SharedHandle<network::Handle>>,
    error_ptr: *mut *mut c_char,
) {
    let session = get();
    let network_handle = session.network.handle();
    session.sender.send_ok(
        port,
        error_ptr,
        SharedHandle::new(std::sync::Arc::new(network_handle)),
    );
}

pub(super) struct Context<'a, T> {
    session: &'a Session,
    port: Port<T>,
    error_ptr: *mut *mut c_char,
}

impl<T> Context<'_, T>
where
    T: Into<DartCObject> + 'static,
{
    pub(super) unsafe fn spawn<F>(self, f: F) -> Result<()>
    where
        F: Future<Output = Result<T>> + Send + 'static,
    {
        self.session
            .runtime
            .spawn(self.session.sender.invoke(self.port, self.error_ptr, f));
        Ok(())
    }

    pub(super) fn network(&self) -> &Network {
        &self.session.network
    }
}

// Utility for sending values to dart.
#[derive(Copy, Clone)]
pub(super) struct Sender {
    post_c_object_fn: PostDartCObjectFn,
}

impl Sender {
    // NOTE: using explicit `impl Future` return instead of `async` to be able to specify the
    // returned future is `Send` even though `error_ptr` is not `Send`.
    unsafe fn invoke<F, T>(
        &self,
        port: Port<T>,
        error_ptr: *mut *mut c_char,
        f: F,
    ) -> impl Future<Output = ()> + Send + 'static
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Into<DartCObject> + 'static,
    {
        let error_ptr = AssumeSend::new(error_ptr);
        let sender = *self;

        async move {
            match f.await {
                Ok(value) => sender.send_ok(port, error_ptr.into_inner(), value),
                Err(error) => sender.send_err(port, error_ptr.into_inner(), error),
            }
        }
    }

    pub(super) unsafe fn send<T>(&self, port: Port<T>, value: T)
    where
        T: Into<DartCObject>,
    {
        (self.post_c_object_fn)(port.into(), &mut value.into());
    }

    unsafe fn send_ok<T>(&self, port: Port<T>, error_ptr: *mut *mut c_char, value: T)
    where
        T: Into<DartCObject>,
    {
        if !error_ptr.is_null() {
            *error_ptr = ptr::null_mut();
        }

        (self.post_c_object_fn)(port.into(), &mut value.into());
    }

    unsafe fn send_err<T, E>(&self, port: Port<T>, error_ptr: *mut *mut c_char, error: E)
    where
        E: fmt::Display,
    {
        if !error_ptr.is_null() {
            *error_ptr = CString::new(error.to_string()).unwrap().into_raw();
        }

        (self.post_c_object_fn)(port.into(), &mut ().into());
    }
}
