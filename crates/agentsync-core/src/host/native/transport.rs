//! Native WebSocket transport — placeholder.
//!
//! The real implementation will move the body of
//! `crate::net::client::open_websocket` and `crate::net::server::Server::bind`
//! into [`NativeTransport`] / [`NativeListener`] during Phase 1.3 of the
//! wasm-parity refactor (the cutover where Vault routes through Host). For
//! Phase 1.1+1.2 the trait surface compiles and is wired into [`Host`], but
//! `Vault` keeps calling the existing `net::client` / `net::server`
//! free functions — so neither method here is reached yet.

use crate::error::{Error, Result};
use crate::host::transport::{Acceptor, Conn, ConnectOpts, Listener, TlsConfig, Transport};
use async_trait::async_trait;
use bytes::Bytes;
use std::net::SocketAddr;

pub struct NativeTransport;

#[async_trait(?Send)]
impl Transport for NativeTransport {
    async fn connect(&self, _url: &str, _opts: ConnectOpts) -> Result<Box<dyn Conn>> {
        Err(Error::Other(
            "NativeTransport::connect not yet wired (cutover lands in Phase 1.3)".into(),
        ))
    }
}

pub struct NativeListener;

#[async_trait(?Send)]
impl Listener for NativeListener {
    async fn bind(&self, _addr: SocketAddr, _tls: Option<TlsConfig>) -> Result<Box<dyn Acceptor>> {
        Err(Error::Other(
            "NativeListener::bind not yet wired (cutover lands in Phase 1.3)".into(),
        ))
    }
}

// Placeholder Conn / Acceptor types kept here to make the trait shapes
// concrete for compilation. They are never instantiated in Phase 1.1+1.2.
#[allow(dead_code)]
struct PlaceholderConn;

#[async_trait(?Send)]
impl Conn for PlaceholderConn {
    async fn send(&mut self, _frame: Bytes) -> Result<()> {
        Err(Error::Other("placeholder".into()))
    }
    async fn recv(&mut self) -> Result<Option<Bytes>> {
        Err(Error::Other("placeholder".into()))
    }
    fn channel_binding(&self) -> Option<[u8; 32]> {
        None
    }
    async fn close(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}
