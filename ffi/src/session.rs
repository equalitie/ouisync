use super::{
    dart::{DartCObject, PostDartCObjectFn},
    directory::Directory,
    error::{ErrorCode, ToErrorCode},
    file::FileHolder,
    logger::Logger,
    registry::{Handle, NullableHandle, Registry},
    repository::RepositoryHolder,
    utils::{self, Bytes, Port, UniqueHandle},
};
use camino::Utf8Path;
use ouisync_lib::{network::Network, ConfigStore, Error, MonitorId, Result, StateMonitor};
use scoped_task::ScopedJoinHandle;
use std::{
    ffi::CString,
    future::Future,
    mem,
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
use tracing::Span;

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

/// Retrieve a serialized state monitor corresponding to the `path`.
#[no_mangle]
pub unsafe extern "C" fn session_get_state_monitor(
    session: SessionHandle,
    path: *const u8,
    path_len: u64,
) -> Bytes {
    let path = std::slice::from_raw_parts(path, path_len as usize);
    let path: Vec<(String, u64)> = match rmp_serde::from_slice(path) {
        Ok(path) => path,
        Err(e) => {
            tracing::error!(
                "Failed to parse input in session_get_state_monitor as MessagePack: {:?}",
                e
            );
            return Bytes::NULL;
        }
    };
    let path = path
        .into_iter()
        .map(|(name, disambiguator)| MonitorId::new(name, disambiguator));

    if let Some(monitor) = session.get().state.root_monitor.locate(path) {
        let bytes = rmp_serde::to_vec(&monitor).unwrap();
        Bytes::from_vec(bytes)
    } else {
        Bytes::NULL
    }
}

/// Subscribe to "on change" events happening inside a monitor corresponding to the `path`.
#[no_mangle]
pub unsafe extern "C" fn session_state_monitor_subscribe(
    session: SessionHandle,
    path: *const u8,
    path_len: u64,
    port: Port<()>,
) -> NullableHandle<ScopedJoinHandle<()>> {
    let path = std::slice::from_raw_parts(path, path_len as usize);
    let path: Vec<(String, u64)> = match rmp_serde::from_slice(path) {
        Ok(path) => path,
        Err(e) => {
            tracing::error!(
                "Failed to parse input in session_state_monitor_subscribe as MessagePack: {:?}",
                e
            );
            return NullableHandle::NULL;
        }
    };
    let path = path
        .into_iter()
        .map(|(name, disambiguator)| MonitorId::new(name, disambiguator));

    let session = session.get();
    let sender = session.sender();
    if let Some(monitor) = session.state.root_monitor.locate(path) {
        let mut rx = monitor.subscribe();

        let handle = session.runtime().spawn(async move {
            loop {
                match rx.changed().await {
                    Ok(()) => sender.send(port, ()),
                    Err(_) => return,
                }
                // Prevent flooding the app with too many "on change" notifications.
                time::sleep(Duration::from_millis(200)).await;
            }
        });
        let handle = ScopedJoinHandle(handle);

        session.state.tasks.insert(handle).into()
    } else {
        NullableHandle::NULL
    }
}

/// Unsubscribe from the above "on change" StateMonitor events.
#[no_mangle]
pub unsafe extern "C" fn session_state_monitor_unsubscribe(
    session: SessionHandle,
    handle: NullableHandle<ScopedJoinHandle<()>>,
) {
    if let Ok(handle) = handle.try_into() {
        session.get().state.tasks.remove(handle);
    }
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

/// Cancel a notification subscription.
#[no_mangle]
pub unsafe extern "C" fn subscription_cancel(
    session: SessionHandle,
    handle: Handle<ScopedJoinHandle<()>>,
) {
    session.get().state.tasks.remove(handle);
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

pub struct Session {
    runtime: Runtime,
    pub(crate) state: Arc<State>,
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

        let session = Session {
            runtime,
            state: Arc::new(State {
                root_monitor,
                repos_span,
                config,
                network,
                repositories: Registry::new(),
                directories: Registry::new(),
                files: Registry::new(),
                tasks: Registry::new(),
            }),
            sender,
            _logger: logger,
        };

        Ok(session)
    }

    /// Start listening for client connections. Returns the listening port.
    pub(crate) async fn listen(&self) -> Result<u16> {
        todo!()
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

    pub(crate) fn sender(&self) -> Sender {
        self.sender
    }
}

pub type SessionHandle = UniqueHandle<Session>;

pub(crate) struct State {
    pub root_monitor: StateMonitor,
    pub repos_span: Span,
    pub config: ConfigStore,
    pub network: Network,
    pub repositories: Registry<RepositoryHolder>,
    pub directories: Registry<Directory>,
    pub files: Registry<FileHolder>,
    pub tasks: Registry<ScopedJoinHandle<()>>,
}

impl State {
    pub(super) fn repo_span(&self, store: &Utf8Path) -> Span {
        tracing::info_span!(parent: &self.repos_span, "repo", ?store)
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

    pub(super) fn state(&self) -> &Arc<State> {
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

    pub(crate) unsafe fn send<T>(&self, port: Port<T>, value: T)
    where
        T: Into<DartCObject>,
    {
        (self.post_c_object_fn)(port.into(), &mut value.into());
    }
}
