//! Transport adapters for the WebSocket layer.
//!
//! `MaybeTlsClientStream` / `MaybeTlsServerStream` wrap either a plain
//! `TcpStream` or a `tokio_rustls::TlsStream`, so the WebSocket framing
//! (and the four-message handshake on top of it) can run over either
//! transport without duplicating the higher-level code paths.
//!
//! The plain variants exist for `--no-tls` deployments where TLS is
//! terminated by an upstream reverse proxy (Fly.io, Railway, etc.) and
//! the agentsync hub speaks plain WS to the proxy. Channel binding
//! degrades in that mode — see [`Server::bind`] / [`ClientConn::connect`]
//! for the security tradeoff.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream as ClientTlsStream;
use tokio_rustls::server::TlsStream as ServerTlsStream;

/// Either a plain TCP stream or a client-side rustls TLS stream wrapping
/// one. Implements [`AsyncRead`] + [`AsyncWrite`] so it can be fed
/// directly into a [`tokio_tungstenite::WebSocketStream`].
pub enum MaybeTlsClientStream {
    Plain(TcpStream),
    Tls(ClientTlsStream<TcpStream>),
}

/// Server-side counterpart of [`MaybeTlsClientStream`].
pub enum MaybeTlsServerStream {
    Plain(TcpStream),
    Tls(ServerTlsStream<TcpStream>),
}

impl AsyncRead for MaybeTlsClientStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTlsClientStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            MaybeTlsClientStream::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTlsClientStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        match self.get_mut() {
            MaybeTlsClientStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            MaybeTlsClientStream::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            MaybeTlsClientStream::Plain(s) => Pin::new(s).poll_flush(cx),
            MaybeTlsClientStream::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            MaybeTlsClientStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            MaybeTlsClientStream::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

impl AsyncRead for MaybeTlsServerStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTlsServerStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            MaybeTlsServerStream::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTlsServerStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        match self.get_mut() {
            MaybeTlsServerStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            MaybeTlsServerStream::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            MaybeTlsServerStream::Plain(s) => Pin::new(s).poll_flush(cx),
            MaybeTlsServerStream::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            MaybeTlsServerStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            MaybeTlsServerStream::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}
