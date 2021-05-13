//! Dart API

// Most of this file is ripped from [dart-sys](https://crates.io/crates/dart-sys) and
// [allo-isolate](https://crates.io/crates/allo-isolate)

#[repr(C)]
#[derive(Copy, Clone)]
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

#[repr(i32)]
#[derive(Copy, Clone)]
pub enum DartCObjectType {
    Null = 0,
    // Bool = 1,
    // Int32 = 2,
    Int64 = 3,
    // Double = 4,
    // String = 5,
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
    pub as_bool: bool,
    // pub as_int32: i32,
    pub as_int64: i64,
    // pub as_double: f64,
    // pub as_string: *mut ::std::os::raw::c_char,
    // NOTE: some variants omitted because we don't currently need them.
    _align: [u64; 5usize],
}

pub type Port = i64;
pub type PostDartCObjectFn = unsafe extern "C" fn(Port, *mut DartCObject) -> bool;
