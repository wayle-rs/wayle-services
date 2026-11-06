//! Shared helpers for unit tests.

use zbus::{Connection, Guid, connection};

/// A peer-to-peer connection pair, for tests that send requests without a
/// message bus; keep the returned peer alive for as long as the connection is
/// used.
pub(crate) async fn connection() -> (Connection, Connection) {
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let guid = Guid::generate();
    let (client, server) = tokio::join!(
        connection::Builder::unix_stream(client).p2p().build(),
        connection::Builder::unix_stream(server)
            .server(guid)
            .unwrap()
            .p2p()
            .build(),
    );
    (client.unwrap(), server.unwrap())
}
