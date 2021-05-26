// Macros to derive the `Type`, `Encode` and `Decode` traits from `sqlx` for types that wrap
// `[u8; N]`. Normally those traits are `#[derive]`-able, but only for types that consist of types
// that already implement those traits. `[u8; N]` doesn't so we need to do it manually.

// Derive `sqlx::Type` for a type that wraps `[u8, N]`.
macro_rules! derive_sqlx_type_for_u8_array_wrapper {
    ($type:ty) => {
        impl sqlx::Type<sqlx::sqlite::Sqlite> for $type {
            fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
                <&[u8] as sqlx::Type<sqlx::sqlite::Sqlite>>::type_info()
            }
        }
    };
}

// Derive `sqlx::Encode` for a type that wraps `[u8, N]`. The type must implement `AsRef<[u8]>`.
macro_rules! derive_sqlx_encode_for_u8_array_wrapper {
    ($type:ty) => {
        impl<'q> sqlx::Encode<'q, sqlx::sqlite::Sqlite> for &'q $type {
            fn encode_by_ref(
                &self,
                args: &mut Vec<sqlx::sqlite::SqliteArgumentValue<'q>>,
            ) -> sqlx::encode::IsNull {
                (*self).as_ref().encode_by_ref(args)
            }
        }
    };
}

// Derive `sqlx::Decode` for a type that wraps `[u8, N]`. The type must implement `TryFrom<&[u8]>`.
macro_rules! derive_sqlx_decode_for_u8_array_wrapper {
    ($type:ty) => {
        impl<'r> sqlx::Decode<'r, sqlx::sqlite::Sqlite> for $type {
            fn decode(
                value: sqlx::sqlite::SqliteValueRef<'r>,
            ) -> Result<Self, sqlx::error::BoxDynError> {
                let slice = <&[u8]>::decode(value)?;
                Ok(slice.try_into()?)
            }
        }
    };
}
