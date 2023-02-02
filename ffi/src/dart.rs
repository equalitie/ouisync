//! Dart API

// Most of this file is ripped from [dart-sys](https://crates.io/crates/dart-sys) and
// [allo-isolate](https://crates.io/crates/allo-isolate)

use super::error::ErrorCode;
use std::{ffi::CString, mem, os::raw::c_char};

#[repr(C)]
pub(crate) struct DartCObject {
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

impl From<u16> for DartCObject {
    fn from(value: u16) -> Self {
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

impl From<Vec<u8>> for DartCObject {
    fn from(value: Vec<u8>) -> Self {
        let mut slice = value.into_boxed_slice();
        let ptr = slice.as_mut_ptr();
        let len = slice.len() as u64;
        mem::forget(slice);

        Self {
            type_: DartCObjectType::TypedData,
            value: DartCObjectValue {
                as_typed_data: DartTypedData {
                    type_: DartTypedDataType::Uint8,
                    length: len as isize,
                    values: ptr,
                },
            },
        }
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
                unsafe {
                    let _ = CString::from_raw(self.value.as_string);
                }
            }
            DartCObjectType::TypedData => {
                // SAFETY: When `type_` is `TypedData` then `value` is a `DartTypedData`. This is
                // guaranteed by construction.
                unsafe {
                    let value = self.value.as_typed_data;

                    match value.type_ {
                        DartTypedDataType::Uint8 => {
                            let _ = Vec::from_raw_parts(
                                value.values,
                                value.length as usize,
                                value.length as usize,
                            );
                        }
                    }
                }
            }
        }
    }
}

#[repr(i32)]
#[derive(Copy, Clone)]
pub(crate) enum DartCObjectType {
    Null = 0,
    Bool = 1,
    Int32 = 2,
    Int64 = 3,
    // Double = 4,
    String = 5,
    // Array = 6,
    TypedData = 7,
    // ExternalTypedData = 8,
    // SendPort = 9,
    // Capability = 10,
    // Unsupported = 11,
    // NumberOfTypes = 12,
}

#[repr(C)]
pub(crate) union DartCObjectValue {
    as_bool: bool,
    as_int32: i32,
    as_int64: i64,
    // as_double: f64,
    as_string: *mut c_char,
    // ...
    as_typed_data: DartTypedData,
    // NOTE: some variants omitted because we don't currently need them.
    _align: [u64; 5usize],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub(crate) struct DartTypedData {
    pub type_: DartTypedDataType,
    pub length: isize,
    pub values: *mut u8,
}

#[repr(i32)]
#[derive(Copy, Clone)]
pub(crate) enum DartTypedDataType {
    // ByteData = 0,
    // Int8 = 1,
    Uint8 = 2,
    // Uint8Clamped = 3,
    // Int16 = 4,
    // Uint16 = 5,
    // Int32 = 6,
    // Uint32 = 7,
    // Int64 = 8,
    // Uint64 = 9,
    // Float32 = 10,
    // Float64 = 11,
    // Float32x4 = 12,
    // Invalid = 13,
}

pub(crate) type Port = i64;
pub(crate) type PostDartCObjectFn = unsafe extern "C" fn(Port, *mut DartCObject) -> bool;
