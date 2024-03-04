pub(crate) mod local;
pub(crate) mod native;

pub(crate) mod tls {
    use rustls::{Certificate, PrivateKey};
    use std::{io, path::Path};
    use tokio::fs;

    pub async fn load_certificates(path: impl AsRef<Path>) -> io::Result<Vec<Certificate>> {
        load_pems(path.as_ref(), "CERTIFICATE")
            .await
            .map(|pems| pems.map(Certificate).collect())
    }

    pub async fn load_keys(path: impl AsRef<Path>) -> io::Result<Vec<PrivateKey>> {
        load_pems(path.as_ref(), "PRIVATE KEY")
            .await
            .map(|pems| pems.map(PrivateKey).collect())
    }

    async fn load_pems<'a>(
        path: &Path,
        tag: &'a str,
    ) -> io::Result<impl Iterator<Item = Vec<u8>> + 'a> {
        let content = fs::read(path).await?;

        pem::parse_many(content)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
            .map(move |pems| {
                pems.into_iter()
                    .filter(move |pem| pem.tag() == tag)
                    .map(|pem| pem.into_contents())
            })
    }
}
