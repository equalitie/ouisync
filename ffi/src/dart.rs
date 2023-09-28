//! Dart API

// Part of this file is ripped from [dart-sys](https://crates.io/crates/dart-sys) and
// [allo-isolate](https://crates.io/crates/allo-isolate)

use crate::sender::Sender;
use bytes::Bytes;
use std::mem;

pub(crate) struct PortSender {
    post_c_object_fn: PostDartCObjectFn,
    port: Port,
}

impl PortSender {
    /// # Safety
    ///
    /// `post_c_object_fn` must be a valid pointer to the `NativeApi.postCObject` dart function.
    pub unsafe fn new(post_c_object_fn: PostDartCObjectFn, port: Port) -> Self {
        Self {
            post_c_object_fn,
            port,
        }
    }
}

impl Sender for PortSender {
    fn send(&self, msg: Bytes) {
        // Safety: `self` must be created via `PortSender::new` and its safety instructions must be
        // followed and `self.post_c_object_fn` can't be modified afterwards.
        unsafe {
            (self.post_c_object_fn)(self.port, &mut msg.into());
        }
    }
}

pub(crate) type Port = i64;
pub(crate) type PostDartCObjectFn = unsafe extern "C" fn(Port, *mut DartCObject) -> bool;

#[repr(C)]
pub(crate) struct DartCObject {
    type_: DartCObjectType,
    value: DartCObjectValue,
}

// TODO: consider using `ExternallyTypedData` to avoid copies
impl From<Bytes> for DartCObject {
    fn from(value: Bytes) -> Self {
        let value = Vec::from(value);
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
    // Null = 0,
    // Bool = 1,
    // Int32 = 2,
    // Int64 = 3,
    // Double = 4,
    // String = 5,
    // Array = 6,
    TypedData = 7,
    // ExternalTypedData = 8,
    // SendPort = 9,
    // Capability = 10,
    // NativePointer = 11,
    // Unsupported = 12,
    // NumberOfTypes = 13,
}

#[repr(C)]
pub(crate) union DartCObjectValue {
    // as_bool: bool,
    // as_int32: i32,
    // as_int64: i64,
    // as_double: f64,
    // as_string: *mut c_char,
    // as_send_port: DartSendPort,
    // as_capability: DartCapability,
    // as_array: DartArray,
    as_typed_data: DartTypedData,
    // as_external_typed_data: DartExternalTypedData,
    // as_native_pointer: DartPointer,
    _align: [u64; 5usize],
}

// #[repr(C)]
// struct DartSendPort {
//     id: Port,
//     origin_id: Port,
// }

// #[repr(C)]
// struct DartCapability {
//     id: i64,
// }

// #[repr(C)]
// struct DartArray {
//     length: isize,
//     values: *mut *mut DartCObject,
// }

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

// #[repr(C)]
// struct DartExternalTypedData {
//     ty: DartTypedDataType,
//     length: isize, // in elements, not bytes
//     data: *mut u8,
//     peer: *mut c_void,
//     callback: DartHandleFinalizer,
// }

// #[repr(C)]
// struct DartPointer {
//     ptr: isize,
//     size: isize,
//     callback: DartHandleFinalizer,
// }

// type DartHandleFinalizer =
//     unsafe extern "C" fn(isolate_callback_data: *mut c_void, peer: *mut c_void);
