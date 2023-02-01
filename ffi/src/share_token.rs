use crate::{
    repository::{access_mode_to_num, ACCESS_MODE_BLIND},
    utils::{self, Bytes},
};
use ouisync_lib::{network, ShareToken};
use std::{os::raw::c_char, ptr, slice, str::FromStr};

/// Returns the access mode of the given share token.
#[no_mangle]
pub unsafe extern "C" fn share_token_mode(token: *const c_char) -> u8 {
    #![allow(clippy::question_mark)] // false positive

    let token = if let Ok(token) = utils::ptr_to_str(token) {
        token
    } else {
        return ACCESS_MODE_BLIND;
    };

    let token: ShareToken = if let Ok(token) = token.parse() {
        token
    } else {
        return ACCESS_MODE_BLIND;
    };

    access_mode_to_num(token.access_mode())
}

/// Returns the info-hash of the repository corresponding to the share token formatted as hex
/// string.
/// User is responsible for deallocating the returned string.
#[no_mangle]
pub unsafe extern "C" fn share_token_info_hash(token: *const c_char) -> *const c_char {
    let token = if let Ok(token) = utils::ptr_to_str(token) {
        token
    } else {
        return ptr::null();
    };

    let token: ShareToken = if let Ok(token) = token.parse() {
        token
    } else {
        return ptr::null();
    };

    utils::str_to_ptr(&hex::encode(
        network::repository_info_hash(token.id()).as_ref(),
    ))
}

/// IMPORTANT: the caller is responsible for deallocating the returned pointer unless it is `null`.
#[no_mangle]
pub unsafe extern "C" fn share_token_suggested_name(token: *const c_char) -> *const c_char {
    let token = if let Ok(token) = utils::ptr_to_str(token) {
        token
    } else {
        return ptr::null();
    };

    let token: ShareToken = if let Ok(token) = token.parse() {
        token
    } else {
        return ptr::null();
    };

    utils::str_to_ptr(token.suggested_name().as_ref())
}

/// Take the input string, decide whether it's a valid OuiSync token and normalize it (remove white
/// space, unnecessary slashes,...).
/// IMPORTANT: the caller is responsible for deallocating the returned buffer unless it is `null`.
#[no_mangle]
pub unsafe extern "C" fn share_token_normalize(token: *const c_char) -> *const c_char {
    #![allow(clippy::question_mark)] // false positive

    let token = if let Ok(token) = utils::ptr_to_str(token) {
        token
    } else {
        return ptr::null();
    };

    let token: ShareToken = if let Ok(token) = ShareToken::from_str(token) {
        token
    } else {
        return ptr::null();
    };

    utils::str_to_ptr(&token.to_string())
}

/// IMPORTANT: the caller is responsible for deallocating the returned buffer unless it is `null`.
#[no_mangle]
pub unsafe extern "C" fn share_token_encode(token: *const c_char) -> Bytes {
    #![allow(clippy::question_mark)] // false positive

    let token = if let Ok(token) = utils::ptr_to_str(token) {
        token
    } else {
        return Bytes::NULL;
    };

    let token: ShareToken = if let Ok(token) = token.parse() {
        token
    } else {
        return Bytes::NULL;
    };

    let mut buffer = Vec::new();
    token.encode(&mut buffer);

    Bytes::from_vec(buffer)
}

/// IMPORTANT: the caller is responsible for deallocating the returned pointer unless it is `null`.
#[no_mangle]
pub unsafe extern "C" fn share_token_decode(bytes: *const u8, len: u64) -> *const c_char {
    let len = if let Ok(len) = len.try_into() {
        len
    } else {
        return ptr::null();
    };

    let slice = slice::from_raw_parts(bytes, len);

    let token = if let Ok(token) = ShareToken::decode(slice) {
        token
    } else {
        return ptr::null();
    };

    utils::str_to_ptr(token.to_string().as_ref())
}
