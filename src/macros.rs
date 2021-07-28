// Macro to create a strongly typed wrapper around a byte array with additional convenient API for
// use as a random unique identifier.
//
// # Example
//
//     define_random_id! {
//         pub struct MyId([u8; 32]);
//     }
//
macro_rules! define_random_id {
    (
        $(#[$attrs:meta])*
        $vis:vis struct $name:ident ( [u8; $size:expr ] );
    ) => {
        $(#[$attrs])*
        #[repr(transparent)]
        #[derive(Copy, Clone, Eq, PartialEq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
        $vis struct $name([u8; $size]);

        impl std::convert::TryFrom<&'_ [u8]> for $name {
            type Error = std::array::TryFromSliceError;

            fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
                use std::convert::TryInto;
                Ok(Self(slice.try_into()?))
            }
        }

        impl AsRef<[u8]> for $name {
            fn as_ref(&self) -> &[u8] {
                &self.0[..]
            }
        }

        impl rand::distributions::Distribution<$name> for rand::distributions::Standard {
            fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> $name {
                $name(self.sample(rng))
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "{:x}", self)
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "{:8x}", self)
            }
        }

        impl std::fmt::LowerHex for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                crate::format::hex(f, &self.0)
            }
        }
    };
}

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
                use std::convert::TryInto;
                let slice = <&[u8]>::decode(value)?;
                Ok(slice.try_into()?)
            }
        }
    };
}

#[cfg(test)]
mod tests {
    define_random_id! {
        struct TestId([u8; 32]);
    }

    #[test]
    fn random_id_fmt() {
        let id = TestId([
            0x00, 0x01, 0x02, 0x03, 0x05, 0x07, 0x0b, 0x0d, 0x11, 0x13, 0x17, 0x1d, 0x1f, 0x25,
            0x29, 0x2b, 0x2f, 0x35, 0x3b, 0x3d, 0x43, 0x47, 0x49, 0x4f, 0x53, 0x59, 0x61, 0x65,
            0x67, 0x6b, 0x6d, 0x71,
        ]);

        assert_eq!(
            format!("{:x}", id),
            "0001020305070b0d1113171d1f25292b2f353b3d4347494f53596165676b6d71"
        );
        assert_eq!(format!("{:1x}", id), "");
        assert_eq!(format!("{:2x}", id), "..");
        assert_eq!(format!("{:3x}", id), "..");
        assert_eq!(format!("{:4x}", id), "00..");
        assert_eq!(format!("{:6x}", id), "0001..");
        assert_eq!(format!("{:8x}", id), "000102..");

        assert_eq!(format!("{:?}", id), "000102..");
        assert_eq!(
            format!("{}", id),
            "0001020305070b0d1113171d1f25292b2f353b3d4347494f53596165676b6d71"
        );
    }
}
