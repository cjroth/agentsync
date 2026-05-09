//! Outbound + inbound WebSocket transport, abstracted away from
//! tokio-tungstenite + tokio_rustls. Native impl wraps the existing
//! `net::transport` types; wasm impl bridges to a JS-supplied callback that
//! returns a duplex of msgpack frames.
//!
//! The transport layer is responsible for terminating TLS (when the URL is
//! `wss://`) and surfacing the negotiated peer certificate fingerprint via
//! [`Conn::channel_binding`]. The handshake transcript builder consumes that
//! fingerprint to bind the auth handshake to the underlying TLS channel.
//! When the transport runs without TLS (plain `ws://` or browser WebSocket
//! — neither exposes peer certs), `channel_binding` returns `None` and the
//! handshake includes an empty fingerprint.

use crate::error::Result;
use crate::identity::Pubkey;
use async_trait::async_trait;
use bytes::Bytes;
use std::net::SocketAddr;

/// Outbound connector. One impl per runtime; multiple [`Conn`]s may be
/// open against a single transport.
#[async_trait(?Send)]
pub trait Transport: Send + Sync + 'static {
    /// Open a new connection. `url` is `wss://host:port` or `ws://host:port`.
    async fn connect(&self, url: &str, opts: ConnectOpts) -> Result<Box<dyn Conn>>;
}

#[derive(Default, Clone)]
pub struct ConnectOpts {
    /// Pin the hub's identity pubkey. The handshake will reject connections
    /// where the hub presents a different pubkey.
    pub expected_hub_pubkey: Option<Pubkey>,
}

/// A single duplex connection over which msgpack-encoded [`crate::Frame`]
/// values flow. Implementations buffer at most one frame internally; back
/// pressure is signalled via `send` returning slowly.
#[async_trait(?Send)]
pub trait Conn: Send + 'static {
    /// Send one binary frame. Errors are non-recoverable; the conn is dead.
    async fn send(&mut self, frame: Bytes) -> Result<()>;
    /// Receive the next frame. `Ok(None)` means the peer closed cleanly.
    async fn recv(&mut self) -> Result<Option<Bytes>>;
    /// SHA-256 of the peer's TLS certificate, if any. Returns `None` when
    /// the underlying transport is plain or doesn't expose the cert (browser
    /// WebSocket). Used by the handshake to bind to the TLS channel.
    fn channel_binding(&self) -> Option<[u8; 32]>;
    /// Cleanly tear down the connection. Best-effort.
    async fn close(self: Box<Self>) -> Result<()>;
}

/// Inbound listener. Native binds a `TcpListener` and optionally wraps with
/// `tokio_rustls`; wasm has no listener at all (only browsers can't listen,
/// but Node could in principle wire `ws.Server` here — left to the host).
#[async_trait(?Send)]
pub trait Listener: Send + Sync + 'static {
    /// Bind to `addr` and return an acceptor. Pass the TLS cert + key DER
    /// when running over TLS; `None` for plain ws://.
    async fn bind(&self, addr: SocketAddr, tls: Option<TlsConfig>) -> Result<Box<dyn Acceptor>>;
}

/// Server-side TLS material. Cert and key are DER-encoded.
pub struct TlsConfig {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

/// Per-bind acceptor that yields one [`Conn`] per inbound connection.
#[async_trait(?Send)]
pub trait Acceptor: Send + 'static {
    /// Wait for the next inbound connection. `Ok(None)` after `close()`.
    async fn accept(&mut self) -> Result<Option<Box<dyn Conn>>>;
    /// The actual bound address (useful when binding to port 0).
    fn local_addr(&self) -> SocketAddr;
    /// Stop accepting new connections; in-flight conns are unaffected.
    async fn close(self: Box<Self>) -> Result<()>;
}
