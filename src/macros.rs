// Macros to derive the `Type`, `Encode` and `Decode` traits from `sqlx` for types that wrap
// `[u8; N]`. Normally those traits are `#[derive]`-able, but only for types that consist of types
// that already implement those traits. `[u8; N]` doesn't so we need to do it manually.
//
// This macro can be used only on types that implement `AsRef<[u8]>` and `TryFrom<&[u8]>`.
macro_rules! derive_sqlx_traits_for_u8_array_wrapper {
    ($type:ty) => {
        impl sqlx::Type<sqlx::sqlite::Sqlite> for $type {
            fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
                <&[u8] as sqlx::Type<sqlx::sqlite::Sqlite>>::type_info()
            }
        }

        impl<'q> sqlx::Encode<'q, sqlx::sqlite::Sqlite> for &'q $type {
            fn encode_by_ref(
                &self,
                args: &mut Vec<sqlx::sqlite::SqliteArgumentValue<'q>>,
            ) -> sqlx::encode::IsNull {
                (*self).as_ref().encode_by_ref(args)
            }
        }

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
