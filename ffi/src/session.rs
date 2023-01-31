use crate::{
    dart::{DartCObject, PostDartCObjectFn},
    error::{ErrorCode, ToErrorCode},
    logger::Logger,
    registry::{Handle, Registry},
    server::Server,
    state::ServerState,
    utils::{self, Bytes, Port, UniqueHandle},
};
use ouisync_lib::{network::Network, ConfigStore, Error, Result, StateMonitor};
use scoped_task::ScopedJoinHandle;
use std::{
    ffi::CString,
    future::Future,
    mem,
    net::SocketAddr,
    os::raw::{c_char, c_void},
    path::PathBuf,
    ptr,
    sync::Arc,
    time::Duration,
};
use tokio::{
    runtime::{self, Runtime},
    time,
};

#[repr(C)]
pub struct SessionOpenResult {
    session: SessionHandle,
    error_code: ErrorCode,
    error_message: *const c_char,
}

impl From<Result<Session>> for SessionOpenResult {
    fn from(result: Result<Session>) -> Self {
        match result {
            Ok(session) => Self {
                session: SessionHandle::new(Box::new(session)),
                error_code: ErrorCode::Ok,
                error_message: ptr::null(),
            },
            Err(error) => Self {
                session: SessionHandle::NULL,
                error_code: error.to_error_code(),
                error_message: utils::str_to_ptr(&error.to_string()),
            },
        }
    }
}

/// Opens the ouisync session. `post_c_object_fn` should be a pointer to the dart's
/// `NativeApi.postCObject` function cast to `Pointer<Void>` (the casting is necessary to work
/// around limitations of the binding generators).
#[no_mangle]
pub unsafe extern "C" fn session_open(
    post_c_object_fn: *const c_void,
    configs_path: *const c_char,
) -> SessionOpenResult {
    let sender = Sender {
        post_c_object_fn: mem::transmute(post_c_object_fn),
    };

    let configs_path = match utils::ptr_to_native_path_buf(configs_path) {
        Ok(configs_path) => configs_path,
        Err(error) => return Err(error).into(),
    };

    Session::create(sender, configs_path).into()
}

/// Starts interface servers listening on a websocket.
#[no_mangle]
pub unsafe extern "C" fn session_start_server(
    session: SessionHandle,
    addr: *const c_char,
    port: Port<Result<ServerHandle>>,
) {
    let session = session.get();
    let state = session.state.clone();
    let sender = session.sender;

    let addr: SocketAddr =
        match utils::ptr_to_str(addr).and_then(|s| s.parse().map_err(|_| Error::MalformedData)) {
            Ok(addr) => addr,
            Err(error) => {
                sender.send_result(port, Err(error));
                return;
            }
        };

    let entry = session.servers.vacant_entry();
    let server_handle = entry.handle();

    let _enter = session.runtime.enter();
    let task = scoped_task::spawn(async move {
        let server = match Server::bind(addr).await {
            Ok(server) => server,
            Err(error) => {
                sender.send_result(port, Err(error));
                return;
            }
        };

        sender.send_result(port, Ok(server_handle));

        server.run(state).await
    });

    entry.insert(task);
}

#[no_mangle]
pub unsafe extern "C" fn session_stop_server(session: SessionHandle, server: ServerHandle) {
    session.get().servers.remove(server);
}

/// Closes the ouisync session.
#[no_mangle]
pub unsafe extern "C" fn session_close(session: SessionHandle) {
    session.release();
}

/// Shutdowns the network and closes the session. This is equivalent to doing it in two steps
/// (`network_shutdown` then `session_close`), but in flutter when the engine is being detached
/// from Android runtime then async wait for `network_shutdown` never completes (or does so
/// randomly), and thus `session_close` is never invoked. My guess is that because the dart engine
/// is being detached we can't do any async await on the dart side anymore, and thus need to do it
/// here.
#[no_mangle]
pub unsafe extern "C" fn session_shutdown_network_and_close(session: SessionHandle) {
    let Session {
        runtime,
        state,
        _logger,
        ..
    } = *session.release();

    runtime.block_on(async move {
        time::timeout(
            Duration::from_millis(500),
            state.network.handle().shutdown(),
        )
        .await
        .unwrap_or(())
    });
}

/// Deallocate string that has been allocated on the rust side
#[no_mangle]
pub unsafe extern "C" fn free_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }

    let _ = CString::from_raw(ptr);
}

/// Deallocate Bytes that has been allocated on the rust side
#[no_mangle]
pub unsafe extern "C" fn free_bytes(bytes: Bytes) {
    let _ = bytes.into_vec();
}

/// Cancel a notification subscription.
pub(crate) fn unsubscribe(state: &ServerState, handle: SubscriptionHandle) {
    state.tasks.remove(handle);
}

pub struct Session {
    runtime: Runtime,
    pub(crate) state: Arc<ServerState>,
    servers: Registry<ScopedJoinHandle<()>>,
    sender: Sender,
    _logger: Logger,
}

impl Session {
    pub(crate) fn create(sender: Sender, configs_path: PathBuf) -> Result<Self> {
        let config = ConfigStore::new(configs_path);

        let root_monitor = StateMonitor::make_root();
        let session_monitor = root_monitor.make_child("Session");
        let panic_counter = session_monitor.make_value::<u32>("panic_counter".into(), 0);

        // Init logger
        let logger =
            Logger::new(panic_counter, root_monitor.clone()).map_err(Error::InitializeLogger)?;

        // Create runtime
        let runtime = runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(Error::InitializeRuntime)?;
        let _enter = runtime.enter(); // runtime context is needed for some of the following calls

        let network = {
            let _enter = tracing::info_span!("Network").entered();
            Network::new(config.clone())
        };

        let repos_span = tracing::info_span!("Repositories");

        let state = Arc::new(ServerState {
            root_monitor,
            repos_span,
            config,
            network,
            repositories: Registry::new(),
            directories: Registry::new(),
            files: Registry::new(),
            tasks: Registry::new(),
        });

        let session = Session {
            runtime,
            state,
            servers: Registry::new(),
            sender,
            _logger: logger,
        };

        Ok(session)
    }

    pub(crate) unsafe fn with<T, F>(&self, port: Port<Result<T>>, f: F)
    where
        F: FnOnce(Context<T>) -> Result<()>,
        T: Into<DartCObject>,
    {
        let context = Context {
            session: self,
            port,
        };
        let _runtime_guard = context.session.runtime.enter();

        match f(context) {
            Ok(()) => (),
            Err(error) => self.sender.send_result(port, Err(error)),
        }
    }

    pub(crate) fn runtime(&self) -> &Runtime {
        &self.runtime
    }
}

pub type SessionHandle = UniqueHandle<Session>;
pub type ServerHandle = Handle<ScopedJoinHandle<()>>;

pub(super) type SubscriptionHandle = Handle<ScopedJoinHandle<()>>;

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

    pub(super) fn state(&self) -> &Arc<ServerState> {
        &self.session.state
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
                tracing::error!("ffi error: {:?}", error);
                (self.post_c_object_fn)(port, &mut error.to_error_code().into());
                (self.post_c_object_fn)(port, &mut error.to_string().into());
            }
        }
    }
}
