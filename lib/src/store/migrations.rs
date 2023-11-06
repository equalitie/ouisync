use super::Error;
use crate::{crypto::sign::Keypair, db};

pub const DATA_VERSION: u64 = 0;

pub(super) async fn run_data(_db: &db::Pool, _write_keys: &Keypair) -> Result<(), Error> {
    Ok(())
}
