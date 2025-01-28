use std::{io, path::Path, sync::Arc};

use tokio::fs;
use tokio_rustls::rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer},
};

use crate::Error;

pub(crate) async fn make_server_config(
    config_dir: &Path,
) -> Result<Arc<rustls::ServerConfig>, Error> {
    let cert_path = config_dir.join("cert.pem");
    let key_path = config_dir.join("key.pem");

    let certs = load_certificates_from_file(&cert_path)
        .await
        .map_err(|error| {
            tracing::error!(
                ?error,
                "failed to load TLS certificate from {}",
                cert_path.display()
            );

            Error::TlsCertificatesNotFound
        })?;

    if certs.is_empty() {
        tracing::error!(
            "failed to load TLS certificate from {}: no certificates found",
            cert_path.display()
        );

        return Err(Error::TlsCertificatesNotFound);
    }

    let keys = load_keys_from_file(&key_path).await.map_err(|error| {
        tracing::error!(?error, "failed to load TLS key from {}", key_path.display(),);

        Error::TlsKeysNotFound
    })?;

    let key = keys.into_iter().next().ok_or_else(|| {
        tracing::error!(
            "failed to load TLS key from {}: no keys found",
            key_path.display()
        );

        Error::TlsKeysNotFound
    })?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|error| {
            tracing::error!(?error, "failed to create server TLS config");
            Error::TlsConfig(error)
        })?;

    Ok(Arc::new(config))
}

pub(crate) async fn make_client_config(
    config_dir: &Path,
) -> Result<Arc<rustls::ClientConfig>, Error> {
    // Load custom root certificates (if any)
    let additional_root_certs = load_certificates_from_dir(&config_dir.join("root_certs"))
        .await
        .inspect_err(|error| tracing::error!(?error, "failed to load TLS root certificates"))
        .map_err(Error::TlsCertificatesInvalid)?;

    let mut root_cert_store = rustls::RootCertStore::empty();

    // Add default root certificates
    root_cert_store.extend(
        webpki_roots::TLS_SERVER_ROOTS
            .iter()
            .map(|ta| ta.to_owned()),
    );

    // Add custom root certificates (if any)
    for cert in additional_root_certs {
        root_cert_store
            .add(cert.clone())
            .inspect_err(|error| tracing::error!(?error, "failed to add TLS root certificate"))
            .map_err(Error::TlsConfig)?;
    }

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();

    Ok(Arc::new(config))
}

/// Loads all certificates in the given directory (non-recursively).
async fn load_certificates_from_dir(dir: &Path) -> io::Result<Vec<CertificateDer<'static>>> {
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
async fn load_certificates_from_file(
    path: impl AsRef<Path>,
) -> io::Result<Vec<CertificateDer<'static>>> {
    load_pems(path.as_ref(), &["CERTIFICATE"])
        .await
        .map(|pems| pems.map(|content| content.into()).collect())
}

/// Loads private keys from the given file.
pub async fn load_keys_from_file(
    path: impl AsRef<Path>,
) -> io::Result<Vec<PrivateKeyDer<'static>>> {
    let pems = load_pems(
        path.as_ref(),
        &["PRIVATE KEY", "EC PRIVATE KEY", "RSA PRIVATE KEY"],
    )
    .await?;

    pems.map(|content| {
        PrivateKeyDer::try_from(content)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    })
    .collect()
}

async fn load_pems<'a>(
    path: &Path,
    tags: &'a [&str],
) -> io::Result<impl Iterator<Item = Vec<u8>> + 'a> {
    let content = fs::read(path).await?;

    let pems = pem::parse_many(content)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    Ok(pems
        .into_iter()
        .filter(move |pem| tags.contains(&pem.tag()))
        .map(|pem| pem.into_contents()))
}
