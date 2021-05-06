use crate::{
    crypto::Cryptor,
    db,
    entry::EntryType,
    error::{Error, Result},
    repository::Repository,
};
use allo_isolate::Isolate;
use std::{
    ffi::CStr,
    os::raw::c_char,
    path::Path,
    ptr,
    sync::{Arc, Once},
};
use tokio::runtime::{self, Runtime};

trait Null {
    const NULL: Self;
}

impl Null for () {
    const NULL: Self = ();
}

impl<T> Null for *const T {
    const NULL: Self = ptr::null();
}

macro_rules! try_ffi {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(error) => {
                log::error!("{}", error);
                return Null::NULL;
            }
        }
    };
}

macro_rules! check_not_null {
    ($ptr:expr) => {
        if $ptr.is_null() {
            return Null::NULL;
        } else {
            &*$ptr
        }
    };
}

/// Opens a repository.
#[no_mangle]
pub unsafe extern "C" fn open_repository(db_path: *const c_char) -> *const Session {
    static LOG_INIT: Once = Once::new();
    LOG_INIT.call_once(env_logger::init);

    log::trace!("open repository");

    let db_path = try_ffi!(to_path(db_path));
    let session = try_ffi!(Session::new(db_path));

    Box::into_raw(Box::new(session))
}

/// Closes a repository.
#[no_mangle]
pub unsafe extern "C" fn close_repository(repo: *const Session) {
    log::trace!("close repository");

    if repo.is_null() {
        return;
    }

    let _ = Box::from_raw(repo as *mut Session);
}

#[no_mangle]
pub unsafe extern "C" fn read_directory(port: i64, repo: *const Session, path: *const c_char) {
    log::trace!("read directory");

    let session = &*check_not_null!(repo);
    let repository = session.repository.clone();
    let path = try_ffi!(to_path(path)).to_owned();
    let isolate = Isolate::new(port);

    session.runtime.spawn(async move {
        let dir = match repository.open_directory(&path).await {
            Ok(dir) => dir,
            Err(error) => {
                log::error!("{}", error);
                isolate.post(false);
                return;
            }
        };

        for entry in dir.entries() {
            log::info!(
                "{}{}",
                entry.name().to_string_lossy(),
                match entry.entry_type() {
                    EntryType::File => "",
                    EntryType::Directory => "/",
                }
            );
        }

        isolate.post(true);
    });
}

pub struct Session {
    repository: Arc<Repository>,
    runtime: Runtime,
}

impl Session {
    fn new(db_path: &Path) -> Result<Self> {
        let runtime = runtime::Builder::new_multi_thread()
            .enable_time()
            .build()
            .map_err(Error::CreateRuntime)?;

        let repository = runtime.block_on(async {
            let pool = db::init(db_path).await?;
            let cryptor = Cryptor::Null;
            Repository::new(pool, cryptor).await
        })?;

        Ok(Self {
            repository: Arc::new(repository),
            runtime,
        })
    }
}

unsafe fn to_path<'a>(path: *const c_char) -> Result<&'a Path> {
    Ok(Path::new(CStr::from_ptr(path).to_str()?))
}
