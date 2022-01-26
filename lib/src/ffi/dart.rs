//! Dart API

// Most of this file is ripped from [dart-sys](https://crates.io/crates/dart-sys) and
// [allo-isolate](https://crates.io/crates/allo-isolate)

use super::error::ErrorCode;
use std::{ffi::CString, os::raw::c_char};

#[repr(C)]
pub struct DartCObject {
    type_: DartCObjectType,
    value: DartCObjectValue,
}

impl From<()> for DartCObject {
    fn from(_: ()) -> Self {
        // Using `false` as a dummy value which should be ignored anyway when type is `Null`.
        DartCObject {
            type_: DartCObjectType::Null,
            value: DartCObjectValue { as_bool: false },
        }
    }
}

impl From<u32> for DartCObject {
    fn from(value: u32) -> Self {
        DartCObject {
            type_: DartCObjectType::Int32,
            value: DartCObjectValue {
                as_int32: value as i32,
            },
        }
    }
}

impl From<u64> for DartCObject {
    fn from(value: u64) -> Self {
        DartCObject {
            type_: DartCObjectType::Int64,
            value: DartCObjectValue {
                as_int64: value as i64,
            },
        }
    }
}

impl From<u8> for DartCObject {
    fn from(value: u8) -> Self {
        DartCObject {
            type_: DartCObjectType::Int32,
            value: DartCObjectValue {
                as_int32: value as i32,
            },
        }
    }
}

impl From<bool> for DartCObject {
    fn from(value: bool) -> Self {
        DartCObject {
            type_: DartCObjectType::Bool,
            value: DartCObjectValue { as_bool: value },
        }
    }
}

impl From<String> for DartCObject {
    fn from(value: String) -> Self {
        DartCObject {
            type_: DartCObjectType::String,
            value: DartCObjectValue {
                as_string: CString::new(value).unwrap_or_default().into_raw(),
            },
        }
    }
}

impl From<ErrorCode> for DartCObject {
    fn from(value: ErrorCode) -> Self {
        Self::from(value as u32)
    }
}

impl Drop for DartCObject {
    fn drop(&mut self) {
        match self.type_ {
            DartCObjectType::Null
            | DartCObjectType::Bool
            | DartCObjectType::Int32
            | DartCObjectType::Int64 => (),
            DartCObjectType::String => {
                // SAFETY: When `type_` is `String` then `value` is a pointer to `CString`. This is
                // guaranteed by construction.
                let _ = unsafe { CString::from_raw(self.value.as_string) };
            }
        }
    }
}

#[repr(i32)]
#[derive(Copy, Clone)]
pub enum DartCObjectType {
    Null = 0,
    Bool = 1,
    Int32 = 2,
    Int64 = 3,
    // Double = 4,
    String = 5,
    // Array = 6,
    // TypedData = 7,
    // ExternalTypedData = 8,
    // SendPort = 9,
    // Capability = 10,
    // Unsupported = 11,
    // NumberOfTypes = 12,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union DartCObjectValue {
    as_bool: bool,
    as_int32: i32,
    as_int64: i64,
    // as_double: f64,
    as_string: *mut c_char,
    // NOTE: some variants omitted because we don't currently need them.
    _align: [u64; 5usize],
}

pub type Port = i64;
pub type PostDartCObjectFn = unsafe extern "C" fn(Port, *mut DartCObject) -> bool;
