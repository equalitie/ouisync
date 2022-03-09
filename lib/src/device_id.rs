use crate::config::{ConfigKey, ConfigStore};
use crate::error::{Error, Result};
use hex::FromHexError;
use rand::{rngs::OsRng, Rng};
use std::{io::ErrorKind, str::FromStr};

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

impl FromStr for DeviceId {
    type Err = FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut buffer = [0; Self::SIZE];
        hex::decode_to_slice(s.trim(), &mut buffer)?;
        Ok(Self(buffer))
    }
}

const KEY: ConfigKey<DeviceId> = ConfigKey::new(
    "device_id",
    "The value stored in this file is the device ID. It is uniquelly generated for each device\n\
     and its only purpose is to detect when a database has been migrated from one device to\n\
     another.\n\
     \n\
     * When a database is migrated, the safest option is to NOT migrate this file with it. *\n\
     \n\
     However, the user may chose to *move* this file alongside the database. In such case it is\n\
     important to ensure the same device ID is never used by a writer replica concurrently from\n\
     more than one location. Doing so will likely result in data loss.\n\
     \n\
     Device ID is never used in construction of network messages and thus can't be used for peer\n\
     identification.",
);

pub async fn get_or_create(config: &ConfigStore) -> Result<DeviceId> {
    let cfg = config.entry(KEY);

    match cfg.get().await {
        Ok(id) => Ok(id),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let new_id = OsRng.gen();
            cfg.set(&new_id).await.map(|_| new_id)
        }
        Err(e) => Err(e),
    }
    .map_err(Error::DeviceIdConfig)
}
