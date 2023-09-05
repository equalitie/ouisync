//! IP geolocation

use maxminddb::{geoip2, MaxMindDBError, Reader};
use std::{
    fmt, io,
    net::IpAddr,
    path::{Path, PathBuf},
    str,
    time::SystemTime,
};
use tokio::{fs::File, io::AsyncReadExt};

pub(crate) struct GeoIp {
    path: PathBuf,
    reader: Option<(Reader<Vec<u8>>, SystemTime)>,
}

impl GeoIp {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            reader: None,
        }
    }

    pub async fn refresh(&mut self) -> Result<(), io::Error> {
        let mut file = File::open(&self.path).await?;

        let new_timestamp = file.metadata().await?.modified()?;
        let old_timestamp = self.reader.as_ref().map(|(_, timestamp)| *timestamp);

        if old_timestamp
            .map(|old_timestamp| old_timestamp < new_timestamp)
            .unwrap_or(true)
        {
            tracing::trace!("Loading GeoIP database");
            let reader = load(&mut file).await?;
            self.reader = Some((reader, new_timestamp));
        }

        Ok(())
    }

    pub fn lookup(&self, addr: IpAddr) -> Result<CountryCode, LookupError> {
        let reader = if let Some((reader, _)) = &self.reader {
            reader
        } else {
            return Err(LookupError::NotFound);
        };

        let country: geoip2::Country = reader.lookup(addr)?;

        Ok(country
            .country
            .ok_or(LookupError::NotFound)?
            .iso_code
            .ok_or(LookupError::NotFound)?
            .into())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub(crate) struct CountryCode([u8; 2]);

impl CountryCode {
    pub const UNKNOWN: Self = CountryCode([b'?'; 2]);

    pub fn as_str(&self) -> &str {
        str::from_utf8(&self.0[..]).unwrap_or("??")
    }
}

impl<'a> From<&'a str> for CountryCode {
    fn from(s: &'a str) -> Self {
        s.as_bytes().try_into().map(Self).unwrap_or(Self::UNKNOWN)
    }
}

impl fmt::Display for CountryCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

pub(crate) enum LookupError {
    NotFound,
    Io(io::Error),
}

impl From<MaxMindDBError> for LookupError {
    fn from(src: MaxMindDBError) -> Self {
        match src {
            MaxMindDBError::AddressNotFoundError(_) => Self::NotFound,
            _ => Self::Io(io::Error::new(io::ErrorKind::Other, src)),
        }
    }
}

async fn load(file: &mut File) -> Result<Reader<Vec<u8>>, io::Error> {
    let mut content = Vec::new();
    file.read_to_end(&mut content).await?;

    let reader = Reader::from_source(content)
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;

    Ok(reader)
}
