use crate::{
    crypto::Cryptor,
    db,
    error::{Error, Result},
    repository::Repository,
};
use std::{ffi::CStr, os::raw::c_char, path::Path, ptr};
use tokio::runtime::{self, Runtime};

#[no_mangle]
pub unsafe extern "C" fn open_repository(db_path: *const c_char) -> *const Session {
    let db_path = match CStr::from_ptr(db_path).to_str() {
        Ok(path) => Path::new(path),
        Err(error) => {
            log::error!("invalid db_path: {}", error);
            return ptr::null();
        }
    };

    let session = match Session::new(db_path) {
        Ok(session) => session,
        Err(error) => {
            log::error!("failed to create session: {}", error);
            return ptr::null();
        }
    };

    Box::into_raw(Box::new(session))
}

#[no_mangle]
pub unsafe extern "C" fn close_repository(repo: *const Session) {
    if repo.is_null() {
        return;
    }

    let _ = Box::from_raw(repo as *mut Session);
}

pub struct Session {
    _repository: Repository,
    _runtime: Runtime,
}

impl Session {
    fn new(db_path: &Path) -> Result<Self> {
        let runtime = runtime::Builder::new_multi_thread()
            .build()
            .map_err(Error::CreateRuntime)?;

        let repository = runtime.block_on(async {
            let pool = db::init(db_path).await?;
            let cryptor = Cryptor::Null;
            Repository::new(pool, cryptor).await
        })?;

        Ok(Self {
            _repository: repository,
            _runtime: runtime,
        })
    }
}
