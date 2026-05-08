use crate::auth::{build_transcript, random_nonce, NONCE_LEN};
use crate::error::{Error, Result};
use crate::identity::{Identity, Pubkey};
use crate::net::protocol::{Frame, HelloOp};
use crate::tls::{cert_fingerprint, client_config_accept_any};
use crate::vault::SyncHandle;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use rustls_pki_types::ServerName;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::task::JoinHandle;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::client_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tracing::{debug, info, warn};

type WsStream = WebSocketStream<TlsStream<TcpStream>>;

/// Establish a TCP+TLS connection to `url` and return the wrapped TlsStream
/// plus the SHA-256 of the cert the server presented.
async fn tls_connect(url: &str) -> Result<(TlsStream<TcpStream>, [u8; 32], String)> {
    let parsed = url::Url::parse(url).map_err(|e| Error::Network(format!("parse url: {}", e)))?;
    if parsed.scheme() != "wss" && parsed.scheme() != "ws" {
        return Err(Error::Network(format!(
            "unsupported scheme {:?} (expected wss://)",
            parsed.scheme()
        )));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| Error::Network("url missing host".into()))?
        .to_string();
    let port = parsed.port().unwrap_or(crate::constants::DEFAULT_PORT);
    let tcp = TcpStream::connect((host.as_str(), port)).await?;

    let connector = TlsConnector::from(client_config_accept_any());
    // Trust comes from the application-layer signature, not from the TLS
    // ServerName, so any name works here. Try IP first; fall back to DNS.
    let server_name = match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ServerName::IpAddress(ip.into()),
        Err(_) => ServerName::try_from(host.clone())
            .map_err(|e| Error::Network(format!("invalid server name: {}", e)))?,
    };
    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| Error::Network(format!("tls connect: {}", e)))?;
    let (_io, conn) = tls.get_ref();
    let peer_certs = conn
        .peer_certificates()
        .ok_or_else(|| Error::Network("server presented no certificate".into()))?;
    let cert_der = peer_certs
        .first()
        .ok_or_else(|| Error::Network("empty server certificate chain".into()))?
        .as_ref()
        .to_vec();
    let fp = cert_fingerprint(&cert_der);
    info!(url, fp = ?hex::encode(fp), "tls handshake complete");
    Ok((tls, fp, url.to_string()))
}

async fn open_websocket(url: &str) -> Result<(WsStream, [u8; 32])> {
    let (tls, fp, _url) = tls_connect(url).await?;
    let request = url
        .into_client_request()
        .map_err(|e| Error::Network(format!("build ws request: {}", e)))?;
    let (ws, _) = client_async(request, tls)
        .await
        .map_err(|e| Error::WebSocket(e.to_string()))?;
    Ok((ws, fp))
}

/// Probe a hub: do the full four-message handshake to learn the hub's
/// vault_id and identity pubkey, then close.
pub async fn discover_vault_id(url: &str, identity: &Identity) -> Result<String> {
    let (vault_id, _hub_pubkey, _vault_name, _ws) = probe_handshake(url, identity).await?;
    Ok(vault_id)
}

async fn probe_handshake(
    url: &str,
    identity: &Identity,
) -> Result<(String, Pubkey, Option<String>, WsStream)> {
    let (ws, cert_fp) = open_websocket(url).await?;
    let (mut writer, mut reader) = ws.split();

    let (vault_id, hub_pubkey, vault_name, _) =
        run_handshake(&mut writer, &mut reader, identity, cert_fp).await?;
    let ws = writer.reunite(reader).map_err(|e| {
        Error::Network(format!("reunite ws after handshake: {}", e))
    })?;
    Ok((vault_id, hub_pubkey, vault_name, ws))
}

async fn run_handshake<S>(
    writer: &mut SplitSink<WebSocketStream<S>, Message>,
    reader: &mut SplitStream<WebSocketStream<S>>,
    identity: &Identity,
    expected_cert_fp: [u8; 32],
) -> Result<(String, Pubkey, Option<String>, Vec<u8>)>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let frame = read_one_frame(reader).await?;
    let (vault_id, hub_pubkey_bytes, hub_nonce_bytes, advertised_fp, vault_name) = match frame {
        Frame::HelloHub {
            vault_id,
            hub_identity_pubkey,
            hub_nonce,
            tls_cert_fingerprint,
            vault_name,
        } => (
            vault_id,
            hub_identity_pubkey,
            hub_nonce,
            tls_cert_fingerprint,
            vault_name,
        ),
        Frame::Error { message } => return Err(Error::Auth(message)),
        _ => return Err(Error::Protocol("expected HelloHub".into())),
    };
    let hub_pubkey = Pubkey::from_bytes(&hub_pubkey_bytes)?;
    if hub_nonce_bytes.len() != NONCE_LEN {
        return Err(Error::Protocol("hub nonce wrong length".into()));
    }
    let mut hub_nonce = [0u8; NONCE_LEN];
    hub_nonce.copy_from_slice(&hub_nonce_bytes);

    // Channel binding: the fingerprint advertised by the hub MUST match the
    // one we observed at the TLS layer. A relayed MITM that re-encrypts to
    // the real listener will trip this — its TLS cert is different from
    // what the hub committed to in HelloHub.
    if advertised_fp != expected_cert_fp.as_slice() {
        return Err(Error::Auth(format!(
            "tls cert fingerprint mismatch: advertised {} bytes, observed {}",
            advertised_fp.len(),
            expected_cert_fp.len()
        )));
    }

    let peer_pubkey = identity.pubkey();
    let peer_nonce = random_nonce();
    let hello_peer = Frame::HelloPeer {
        peer_identity_pubkey: peer_pubkey.as_bytes().to_vec(),
        peer_nonce: peer_nonce.to_vec(),
        op: HelloOp::Join,
    };
    writer.send(Message::binary(hello_peer.encode()?)).await?;

    let transcript = build_transcript(
        &hub_nonce,
        &peer_nonce,
        &advertised_fp,
        hub_pubkey.as_bytes(),
        peer_pubkey.as_bytes(),
    );
    let frame = read_one_frame(reader).await?;
    let hub_sig = match frame {
        Frame::ProofHub { sig } => sig,
        Frame::Error { message } => return Err(Error::Auth(message)),
        _ => return Err(Error::Protocol("expected ProofHub".into())),
    };
    if !hub_pubkey.verify(&transcript, &hub_sig) {
        return Err(Error::Auth("hub signature failed verification".into()));
    }

    let peer_sig = identity.sign(&transcript).await?;
    let proof_peer = Frame::ProofPeer {
        sig: peer_sig.to_vec(),
    };
    writer.send(Message::binary(proof_peer.encode()?)).await?;

    Ok((vault_id, hub_pubkey, vault_name, transcript))
}

pub struct ClientConn {
    close_tx: Option<oneshot::Sender<()>>,
    writer_task: Option<JoinHandle<()>>,
    reader_task: Option<JoinHandle<()>>,
    notif_task: Option<JoinHandle<()>>,
    sync_handle: Arc<dyn SyncHandle>,
    peer_id: u64,
    pub vault_id: String,
    pub hub_pubkey: Pubkey,
    /// The remote vault's display name from its `[vault] name` config, if any.
    /// `None` for hubs predating the field.
    pub vault_name: Option<String>,
    closed_notify: Arc<Notify>,
    is_closed: Arc<AtomicBool>,
}

impl ClientConn {
    pub async fn wait_closed(&self) {
        self.closed_signal().wait().await;
    }

    pub fn closed_signal(&self) -> ClosedSignal {
        ClosedSignal {
            notify: self.closed_notify.clone(),
            flag: self.is_closed.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ClosedSignal {
    notify: Arc<Notify>,
    flag: Arc<AtomicBool>,
}

impl ClosedSignal {
    pub async fn wait(&self) {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.flag.load(Ordering::Acquire) {
            return;
        }
        notified.await;
    }
}

impl ClientConn {
    pub async fn close(mut self) {
        if let Some(tx) = self.close_tx.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.reader_task.take() {
            h.abort();
        }
        if let Some(h) = self.notif_task.take() {
            h.abort();
        }
        if let Some(h) = self.writer_task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), h).await;
        }
        self.sync_handle.unregister_peer(self.peer_id).await;
    }
}

impl Drop for ClientConn {
    fn drop(&mut self) {
        if let Some(tx) = self.close_tx.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.reader_task.take() {
            h.abort();
        }
        if let Some(h) = self.notif_task.take() {
            h.abort();
        }
    }
}

impl ClientConn {
    pub async fn connect(
        url: &str,
        expected_vault_id: Option<String>,
        expected_hub_pubkey: Option<Pubkey>,
        identity: Identity,
        sync_handle: Arc<dyn SyncHandle>,
    ) -> Result<Self> {
        let (ws, cert_fp) = open_websocket(url).await?;
        info!(url, "connected to rendezvous");
        let (mut writer, mut reader) = ws.split();

        let (vault_id, hub_pubkey, vault_name, _) =
            run_handshake(&mut writer, &mut reader, &identity, cert_fp).await?;

        if let Some(expected) = &expected_vault_id {
            if expected != &vault_id {
                return Err(Error::Auth(format!(
                    "vault_id mismatch: server reported {} but local config has {}",
                    vault_id, expected
                )));
            }
        }

        if let Some(expected) = expected_hub_pubkey {
            if expected != hub_pubkey {
                return Err(Error::Auth(format!(
                    "hub identity mismatch: pinned {} but hub presented {}.\n\
                     Either the hub's identity key was rotated, or someone is \
                     impersonating it. Run `agentsync hub trust {}` to accept the \
                     new key, or `agentsync hub forget` to clear the pin.",
                    expected.fingerprint_sha256(),
                    hub_pubkey.fingerprint_sha256(),
                    hub_pubkey.to_ssh_string()
                )));
            }
        }

        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Frame>();
        let peer_id = sync_handle
            .register_peer(out_tx.clone(), Some(hub_pubkey))
            .await?;

        if let Some(msg) = sync_handle.generate_sync_message(peer_id).await? {
            let _ = out_tx.send(Frame::Sync { bytes: msg });
        }

        let (close_tx, mut close_rx) = oneshot::channel::<()>();
        let writer_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = &mut close_rx => break,
                    frame = out_rx.recv() => match frame {
                        Some(frame) => {
                            let bytes = match frame.encode() {
                                Ok(b) => b,
                                Err(e) => {
                                    warn!(error=%e, "encode frame");
                                    continue;
                                }
                            };
                            if writer.send(Message::binary(bytes)).await.is_err() {
                                return;
                            }
                        }
                        None => break,
                    }
                }
            }
            let _ = writer.send(Message::Close(None)).await;
            let _ = writer.close().await;
        });

        let sync_for_reader = sync_handle.clone();
        let out_for_reader = out_tx.clone();
        let closed_notify = Arc::new(Notify::new());
        let is_closed = Arc::new(AtomicBool::new(false));
        let reader_closed_notify = closed_notify.clone();
        let reader_is_closed = is_closed.clone();
        let reader_task = tokio::spawn(async move {
            while let Some(msg) = reader.next().await {
                let bytes = match msg {
                    Ok(Message::Binary(b)) => b,
                    Ok(Message::Close(_)) => break,
                    Ok(_) => continue,
                    Err(e) => {
                        warn!(error=%e, "ws read error");
                        break;
                    }
                };
                let frame = match Frame::decode(&bytes) {
                    Ok(f) => f,
                    Err(e) => {
                        warn!(error=%e, "decode frame");
                        continue;
                    }
                };
                handle_inbound(peer_id, frame, &sync_for_reader, &out_for_reader).await;
            }
            sync_for_reader.unregister_peer(peer_id).await;
            reader_is_closed.store(true, Ordering::Release);
            reader_closed_notify.notify_waiters();
        });

        let sync_for_notif = sync_handle.clone();
        let out_for_notif = out_tx.clone();
        let notif_task = tokio::spawn(async move {
            loop {
                match sync_for_notif.generate_sync_message(peer_id).await {
                    Ok(Some(bytes)) => {
                        if out_for_notif.send(Frame::Sync { bytes }).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        warn!(error=%e, "generate sync");
                        break;
                    }
                }
                tokio::select! {
                    _ = sync_for_notif.wait_doc_changed() => {}
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                }
            }
        });

        Ok(ClientConn {
            close_tx: Some(close_tx),
            writer_task: Some(writer_task),
            reader_task: Some(reader_task),
            notif_task: Some(notif_task),
            sync_handle,
            peer_id,
            vault_id,
            hub_pubkey,
            vault_name,
            closed_notify,
            is_closed,
        })
    }
}

pub(crate) async fn handle_inbound(
    peer_id: u64,
    frame: Frame,
    sync_handle: &Arc<dyn SyncHandle>,
    out_tx: &mpsc::UnboundedSender<Frame>,
) {
    match frame {
        Frame::Sync { bytes } => match sync_handle.receive_sync_message(peer_id, &bytes).await {
            Ok(_) => {
                if let Ok(Some(reply)) = sync_handle.generate_sync_message(peer_id).await {
                    let _ = out_tx.send(Frame::Sync { bytes: reply });
                }
            }
            Err(e) => warn!(error=%e, "receive sync"),
        },
        Frame::BlobFetch { hash } => match sync_handle.read_blob(&hash).await {
            Ok(bytes) => {
                let _ = out_tx.send(Frame::BlobPush { hash, bytes });
            }
            Err(e) => {
                let _ = out_tx.send(Frame::Error {
                    message: format!("blob fetch {}: {}", hash, e),
                });
            }
        },
        Frame::BlobPush { hash, bytes } => {
            if let Err(e) = sync_handle.write_blob(&hash, &bytes).await {
                warn!(error=%e, "write blob");
            }
        }
        Frame::Ping { ts } => {
            let _ = out_tx.send(Frame::Pong { ts });
        }
        Frame::Pong { .. } => {}
        Frame::Error { message } => warn!(message, "peer error"),
        Frame::HelloHub { .. }
        | Frame::HelloPeer { .. }
        | Frame::ProofHub { .. }
        | Frame::ProofPeer { .. } => {
            debug!("ignoring late handshake frame");
        }
    }
}

async fn read_one_frame<S>(
    reader: &mut SplitStream<WebSocketStream<S>>,
) -> Result<Frame>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        match reader.next().await {
            Some(Ok(Message::Binary(b))) => return Frame::decode(&b),
            Some(Ok(Message::Close(_))) => {
                return Err(Error::Network("connection closed mid-handshake".into()));
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(Error::WebSocket(e.to_string())),
            None => return Err(Error::Network("stream ended mid-handshake".into())),
        }
    }
}
