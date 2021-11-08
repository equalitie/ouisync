use super::{
    session,
    utils::{Port, SharedHandle},
};
use crate::network;
use std::os::raw::{c_char, c_uint};

#[no_mangle]
pub unsafe extern "C" fn network_is_local_discovery_enabled(
    network_handle: SharedHandle<network::Handle>,
    port: Port<bool>,
    error_ptr: *mut *mut c_char,
) {
    session::with(port, error_ptr, |ctx| {
        ctx.spawn(async move { Ok(network_handle.get().is_local_discovery_enabled().await) })
    })
}

#[no_mangle]
pub unsafe extern "C" fn network_enable_local_discovery(
    network_handle: SharedHandle<network::Handle>,
    enable: c_uint, // there is no c_bool
    port: Port<()>,
    error_ptr: *mut *mut c_char,
) {
    session::with(port, error_ptr, |ctx| {
        ctx.spawn(async move {
            network_handle
                .get()
                .enable_local_discovery(enable > 0)
                .await;
            Ok(())
        })
    })
}
