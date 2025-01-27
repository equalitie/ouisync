//! Utilities for handling TLS certificates.

use std::{io, path::Path};
use tokio::fs;
use tokio_rustls::rustls::{Certificate, PrivateKey};

/// Loads all certificates in the given directory (non-recursively).
pub async fn load_certificates_from_dir(dir: &Path) -> io::Result<Vec<Certificate>> {
    let mut read_dir = match fs::read_dir(dir).await {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };

    let mut certs = Vec::new();

    while let Some(entry) = read_dir.next_entry().await? {
        if !entry.file_type().await?.is_file() {
            continue;
        }

        let path = entry.path();

        match path.extension().and_then(|e| e.to_str()) {
            Some("pem" | "crt") => (),
            Some(_) | None => continue,
        }

        certs.extend(load_certificates_from_file(entry.path()).await?);
    }

    Ok(certs)
}

/// Loads certificates from the given file.
pub async fn load_certificates_from_file(path: impl AsRef<Path>) -> io::Result<Vec<Certificate>> {
    load_pems(path.as_ref(), &["CERTIFICATE"])
        .await
        .map(|pems| pems.map(Certificate).collect())
}

/// Loads private keys from the given file.
pub async fn load_keys_from_file(path: impl AsRef<Path>) -> io::Result<Vec<PrivateKey>> {
    load_pems(
        path.as_ref(),
        &["PRIVATE KEY", "EC PRIVATE KEY", "RSA PRIVATE KEY"],
    )
    .await
    .map(|pems| pems.map(PrivateKey).collect())
}

async fn load_pems<'a>(
    path: &Path,
    tags: &'a [&'a str],
) -> io::Result<impl Iterator<Item = Vec<u8>> + 'a> {
    let content = fs::read(path).await?;

    pem::parse_many(content)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
        .map(move |pems| {
            pems.into_iter()
                .filter(move |pem| tags.contains(&pem.tag()))
                .map(|pem| pem.into_contents())
        })
}
