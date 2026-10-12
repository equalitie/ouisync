use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::protocol::API_PROTOCOL_VERSION;

use super::net::LocalStream;

// # Protocol version negotiation
//
// ## Definitions
//
// `client_supported_versions`: list of protocol versions the client supports
// `server_supported_versions`: list of protocol versions the server supports
// `client_version`:            protocol version sent by the client to the server
// `server_version`:            protocol version sent by the server to the client
//
// The versions are monotonically increasing unsigned integers starting at 1, serialized as
// big-endian u64.
//
// ## The protocol
//
// 1. Client: send `max(client_supported_versions)` to the server
// 2. Server: compare `client_version` against `server_supported_versions`:
//     - if `client_version` is in `server_supported_versions`, send the same version back to the
//       client
//     - if `client_version` is less than `min(server_supported_versions)`, send
//       `min(server_supported_versions)`
//     - if `client_version` is greater than `max(server_supported_versions)`, send
//       `max(server_supported_versions)`
// 3. Client: if `server_version` is in `client_supported_versions`, proceed with the connection
//    using that version. Otherwise, return error and terminate the connection (`server_version`
//    can be included in the error message)

pub(super) async fn client(stream: &mut LocalStream) -> Result<u64, io::Error> {
    let client_version = API_PROTOCOL_VERSION;
    stream.write_u64(client_version).await?;

    let server_version = stream.read_u64().await?;

    if client_version == server_version {
        Ok(server_version)
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "unsupported protocol version (client: {client_version}, server: {server_version})"
            ),
        ))
    }
}

pub(super) async fn server(stream: &mut LocalStream) -> Result<(), io::Error> {
    let server_version = API_PROTOCOL_VERSION;
    let _client_version = stream.read_u64().await?;
    stream.write_u64(server_version).await?;

    Ok(())
}
