use crate::error::{Error, Result};
use rand::{rngs::OsRng, Rng};
use std::io::{self, ErrorKind};
use std::path::Path;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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
    match File::open(path).await {
        Ok(file) => parse(file).await.map_err(Error::DeviceIdConfig),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            create(path).await.map_err(Error::DeviceIdConfig)
        }
        Err(e) => Err(Error::DeviceIdConfig(e)),
    }
}

pub async fn parse(file: File) -> io::Result<DeviceId> {
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();

        if line.is_empty() || line.starts_with("#") {
            continue;
        }

        let bytes = match hex::decode(line) {
            Ok(bytes) => bytes,
            Err(e) => {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    format!("failed to parse config line {:?}: {:?}", line, e),
                ))
            }
        };

        return Ok(DeviceId(bytes.try_into().unwrap()));
    }

    Err(io::Error::new(
        ErrorKind::InvalidData,
        "no device ID found in the device ID config file",
    ))
}

const COMMENT: &'static str = "\
# The value stored in this file is the device ID. It is uniquelly generated for each device and
# its only purpose is to detect when a database has been migrated from one device to another.
#
# * When a database is migrated, the safest option is to NOT migrate this file with it.*
#
# However, the user may chose to *move* this file alongside the database. In such case it is
# important to ensure the same device ID is never used by a writer replica concurrently from
# more than one location. Doing so will likely result in data corruption.
#
# Device ID is never transmitted over the network and thus can't be used for peer identification.";

pub async fn create(path: &Path) -> std::io::Result<DeviceId> {
    // TODO: Consider doing this atomically by first writing to a .tmp file and then rename once
    // writing is done.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await?;
    let id = OsRng.gen::<DeviceId>();
    let content = format!("{}\n\n{}\n", COMMENT, hex::encode(id.as_ref()));
    file.write_all(content.as_ref()).await?;
    Ok(id)
}
