use super::{
    dart::{DartCObject, PostDartCObjectFn},
    error::{ErrorCode, ToErrorCode},
    logger::Logger,
    utils::{self, Bytes, Port, UniqueHandle, UniqueNullableHandle},
};
use crate::{
    config::ConfigStore,
    device_id::{self, DeviceId},
    error::{Error, Result},
    network::{Network, NetworkOptions},
};
use std::{
    future::Future,
    mem,
    os::raw::{c_char, c_void},
    ptr, thread,
};
use tokio::{
    runtime::{self, Runtime},
    task::JoinHandle,
};

/// Opens the ouisync session. `post_c_object_fn` should be a pointer to the dart's
/// `NativeApi.postCObject` function cast to `Pointer<Void>` (the casting is necessary to work
/// around limitations of the binding generators).
#[no_mangle]
pub unsafe extern "C" fn session_open(
    post_c_object_fn: *const c_void,
    configs_path: *const c_char,
    port: Port<Result<()>>,
) {
    let sender = Sender {
        post_c_object_fn: mem::transmute(post_c_object_fn),
    };

    if !SESSION.is_null() {
        // Session already exists.
        sender.send_result(port, Ok(()));
        return;
    }

    // Init logger
    let logger = match Logger::new() {
        Ok(logger) => logger,
        Err(error) => {
            sender.send_result(port, Err(Error::InitializeLogger(error)));
            return;
        }
    };

    let runtime = match runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            sender.send_result(port, Err(Error::InitializeRuntime(error)));
            return;
        }
    };

    let configs_path = match utils::ptr_to_native_path_buf(configs_path) {
        Ok(configs_path) => configs_path,
        Err(error) => {
            sender.send_result(port, Err(error));
            return;
        }
    };

    let config = ConfigStore::new(configs_path);

    // NOTE: spawning a separate thread and using `runtime.block_on` instead of using
    // `runtime.spawn` to avoid moving the runtime into "itself" which would be problematic because
    // if there was an error the runtime would be dropped inside an async context which would panic.
    thread::spawn(move || {
        let device_id = match runtime.block_on(device_id::get_or_create(&config)) {
            Ok(device_id) => device_id,
            Err(error) => {
                sender.send_result(port, Err(error));
                return;
            }
        };

        let network = match runtime.block_on(Network::new(&NetworkOptions::default(), config)) {
            Ok(network) => network,
            Err(error) => {
                sender.send_result(port, Err(error.into()));
                return;
            }
        };

        let session = Session {
            runtime,
            device_id,
            network,
            sender,
            _logger: logger,
        };

        assert!(SESSION.is_null());
        SESSION = Box::into_raw(Box::new(session));

        sender.send_result(port, Ok(()));
    });
}

/// Retrieve a serialized state monitor corresponding to the `path`.  The path is in the form
/// "a:b:c". An empty string returns the "root" state monitor.
#[no_mangle]
pub unsafe extern "C" fn session_get_state_monitor(path: *const c_char) -> Bytes {
    let path = match utils::ptr_to_str(path) {
        Ok(s) => s,
        Err(e) => {
            log::error!(
                "Failed to parse input in session_get_state_monitor: {:?}",
                e
            );
            return Bytes::NULL;
        }
    };

    if let Some(monitor) = get().network.state_monitor.locate(path) {
        let bytes = rmp_serde::to_vec(&*monitor).unwrap();
        Bytes::from_vec(bytes)
    } else {
        Bytes::NULL
    }
}

/// Subscribe to "on change" events happening inside a monitor corresponding to the `path`.  The
/// path is in the form "a:b:c" and an empty string represents the "root" state monitor.
#[no_mangle]
pub unsafe extern "C" fn session_state_monitor_subscribe(
    path: *const c_char,
    port: Port<()>,
) -> UniqueNullableHandle<JoinHandle<()>> {
    let path = match utils::ptr_to_str(path) {
        Ok(s) => s,
        Err(e) => {
            log::error!(
                "Failed to parse input in session_get_state_monitor: {:?}",
                e
            );
            return UniqueNullableHandle::NULL;
        }
    };

    let session = get();
    let sender = session.sender();

    if let Some(monitor) = get().network.monitor.locate(path) {
        let mut rx = monitor.subscribe();

        let handle = session.runtime().spawn(async move {
            loop {
                match rx.changed().await {
                    Ok(()) => sender.send(port, ()),
                    Err(_) => return,
                }
            }
        });

        UniqueNullableHandle::new(Box::new(handle))
    } else {
        UniqueNullableHandle::NULL
    }
}

/// Unsubscribe from the above "on change" StateMonitor events.
#[no_mangle]
pub unsafe extern "C" fn session_state_monitor_unsubscribe(
    handle: UniqueNullableHandle<JoinHandle<()>>,
) {
    if let Some(handle) = handle.release() {
        handle.abort();
    }
}

/// Closes the ouisync session.
#[no_mangle]
pub unsafe extern "C" fn session_close() {
    let session = mem::replace(&mut SESSION, ptr::null_mut());
    if !session.is_null() {
        Box::from_raw(session);
    }
}

/// Cancel a notification subscription.
#[no_mangle]
pub unsafe extern "C" fn subscription_cancel(handle: UniqueHandle<JoinHandle<()>>) {
    handle.release().abort();
}

pub(super) unsafe fn with<T, F>(port: Port<Result<T>>, f: F)
where
    F: FnOnce(Context<T>) -> Result<()>,
    T: Into<DartCObject>,
{
    assert!(!SESSION.is_null(), "session is not initialized");

    let session = &*SESSION;
    let context = Context { session, port };

    let _runtime_guard = context.session.runtime.enter();

    match f(context) {
        Ok(()) => (),
        Err(error) => session.sender.send_result(port, Err(error)),
    }
}

pub(super) unsafe fn get<'a>() -> &'a Session {
    assert!(!SESSION.is_null(), "session is not initialized");
    &*SESSION
}

static mut SESSION: *mut Session = ptr::null_mut();

pub(super) struct Session {
    runtime: Runtime,
    device_id: DeviceId,
    network: Network,
    sender: Sender,
    _logger: Logger,
}

impl Session {
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

pub(super) struct Context<'a, T> {
    session: &'a Session,
    port: Port<Result<T>>,
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
            .spawn(self.session.sender.invoke(self.port, f));
        Ok(())
    }

    pub(super) fn network(&self) -> &Network {
        &self.session.network
    }

    pub(super) fn device_id(&self) -> &DeviceId {
        &self.session.device_id
    }
}

// Utility for sending values to dart.
#[derive(Copy, Clone)]
pub(super) struct Sender {
    post_c_object_fn: PostDartCObjectFn,
}

impl Sender {
    pub(crate) async unsafe fn invoke<F, T>(self, port: Port<Result<T>>, f: F)
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Into<DartCObject> + 'static,
    {
        self.send_result(port, f.await)
    }

    pub(crate) unsafe fn send_result<T>(&self, port: Port<Result<T>>, value: Result<T>)
    where
        T: Into<DartCObject>,
    {
        let port = port.into();

        match value {
            Ok(value) => {
                (self.post_c_object_fn)(port, &mut ErrorCode::Ok.into());
                (self.post_c_object_fn)(port, &mut value.into());
            }
            Err(error) => {
                log::error!("ffi error: {:?}", error);
                (self.post_c_object_fn)(port, &mut error.to_error_code().into());
                (self.post_c_object_fn)(port, &mut error.to_string().into());
            }
        }
    }

    pub(crate) unsafe fn send<T>(&self, port: Port<T>, value: T)
    where
        T: Into<DartCObject>,
    {
        (self.post_c_object_fn)(port.into(), &mut value.into());
    }
}
