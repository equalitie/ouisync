use super::{
    session,
    utils::{Port, SharedHandle},
};
use crate::{crypto::Cryptor, repository::Repository};
use std::{os::raw::c_char, sync::Arc};

/// Opens a repository.
///
/// NOTE: eventually this function will allow to specify which repository to open, but currently
/// only one repository is supported.
#[no_mangle]
pub unsafe extern "C" fn repository_open(
    port: Port<SharedHandle<Repository>>,
    error: *mut *mut c_char,
) {
    session::with(port, error, |ctx| {
        let pool = ctx.pool().clone();
        let cryptor = Cryptor::Null; // TODO: support encryption

        ctx.spawn(async move {
            let repo = Repository::new(pool, cryptor).await?;
            let repo = Arc::new(repo);
            Ok(SharedHandle::new(repo))
        })
    })
}

/// Closes a repository.
#[no_mangle]
pub unsafe extern "C" fn repository_close(handle: SharedHandle<Repository>) {
    handle.release();
}
