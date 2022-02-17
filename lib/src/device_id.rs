use crate::error::{Error, Result};
use rand::{rngs::OsRng, Rng};
use std::io::{self, ErrorKind};
use std::path::Path;
use crate::single_value_config::SingleValueConfig;

define_byte_array_wrapper! {
    /// DeviceId uniquely identifies machines on which this software is running. Its only purpose is
    /// to ensure that one WriterId (which currently equates to sing::PublicKey) will never create two
    /// or more concurrent snapshots as that would break the whole repository.  This is achieved by
    /// ensuring that there is always only a single DeviceId associated with a single WriterId.
    ///
    /// This means that whenever the database is copied/moved from one device to another, the database
    /// containing the DeviceId MUST either not be migrated with it, OR ensure that it'll never be
    /// used from its original place.
    ///
    /// ReplicaIds are private and thus not shared over the network.
    pub struct DeviceId([u8; 32]);
}

derive_rand_for_wrapper!(DeviceId);
derive_sqlx_traits_for_byte_array_wrapper!(DeviceId);

pub async fn get_or_create(path: &Path) -> Result<DeviceId> {
    let cfg = SingleValueConfig::new(path, CONFIG_COMMENT);

    match cfg.get::<String>().await {
        Ok(string) => hex_decode(string),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let new_id = OsRng.gen::<DeviceId>();
            cfg.set(&hex::encode(new_id.as_ref())).await.map(|_| new_id)
        },
        Err(e) => Err(e)
    }
    .map_err(Error::DeviceIdConfig)
}

fn hex_decode(hex: String) -> io::Result<DeviceId> {
    let bytes = match hex::decode(&hex) {
        Ok(bytes) => bytes,
        Err(e) => {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("failed to decode from hex {:?}: {:?}", hex, e),
            ))
        }
    };
    
    match bytes.try_into() {
        Ok(bytes) => Ok(DeviceId(bytes)),
        Err(e) => Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("device ID has incorrect size {:?}: {:?}", hex, e),
        )),
    }
}

const CONFIG_COMMENT: &str = "\
# The value stored in this file is the device ID. It is uniquelly generated for each device and
# its only purpose is to detect when a database has been migrated from one device to another.
#
# * When a database is migrated, the safest option is to NOT migrate this file with it.*
#
# However, the user may chose to *move* this file alongside the database. In such case it is
# important to ensure the same device ID is never used by a writer replica concurrently from
# more than one location. Doing so will likely result in data loss.
#
# Device ID is never used in construction of network messages and thus can't be used for peer
# identification.";
