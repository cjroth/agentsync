use crate::auth::{derive_auth_token, VaultKey};
use crate::error::{Error, Result};
use crate::net::protocol::{Frame, HelloOp};
use crate::vault::SyncHandle;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tracing::{debug, info, warn};

/// One-shot connect to ask the server what its vault_id is. Used by clients
/// that only have a rendezvous URL + key and want to discover the vault to
/// join. The returned `vault_id` should be persisted locally so subsequent
/// runs can validate the connection.
pub async fn discover_vault_id(url: &str, vault_key: VaultKey) -> Result<String> {
    let (ws, _) = connect_async(url).await?;
    let (mut writer, mut reader) = ws.split();

    let hello = Frame::Hello {
        vault_id: None,
        auth_token: derive_auth_token(&vault_key).to_vec(),
        op: HelloOp::Join,
    };
    writer.send(Message::binary(hello.encode()?)).await?;

    while let Some(msg) = reader.next().await {
        let bytes = match msg {
            Ok(Message::Binary(b)) => b,
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(e) => return Err(Error::WebSocket(e.to_string())),
        };
        let frame = Frame::decode(&bytes)?;
        match frame {
            Frame::HelloAck { vault_id } => {
                let _ = writer.send(Message::Close(None)).await;
                let _ = writer.close().await;
                return Ok(vault_id);
            }
            Frame::Error { message } => {
                return Err(Error::Auth(message));
            }
            _ => continue,
        }
    }
    Err(Error::Network("no HelloAck received".into()))
}

pub struct ClientConn {
    close_tx: Option<oneshot::Sender<()>>,
    writer_task: Option<JoinHandle<()>>,
    reader_task: Option<JoinHandle<()>>,
    notif_task: Option<JoinHandle<()>>,
    sync_handle: Arc<dyn SyncHandle>,
    peer_id: u64,
}

impl ClientConn {
    /// Send a WebSocket Close frame to the peer and tear down the connection.
    /// The writer task is awaited (with a short timeout) so the Close frame
    /// actually goes out on the wire before the underlying socket closes.
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
        // Idempotent — the reader task may have already done this on a
        // server-initiated close.
        self.sync_handle.unregister_peer(self.peer_id).await;
    }
}

impl Drop for ClientConn {
    fn drop(&mut self) {
        // Best-effort: nudge the writer task so it has a chance to flush the
        // Close frame before the runtime tears tasks down. Drop is sync so we
        // can't await; callers who care about graceful close should call
        // `close().await`.
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
        vault_id: String,
        vault_key: VaultKey,
        sync_handle: Arc<dyn SyncHandle>,
    ) -> Result<Self> {
        let (ws, _) = connect_async(url).await?;
        info!(url, "connected to rendezvous");
        let (mut writer, mut reader) = ws.split();

        // HELLO
        let hello = Frame::Hello {
            vault_id: Some(vault_id.clone()),
            auth_token: derive_auth_token(&vault_key).to_vec(),
            op: HelloOp::Join,
        };
        writer.send(Message::binary(hello.encode()?)).await?;

        // Wait for HelloAck.
        loop {
            match reader.next().await {
                Some(Ok(Message::Binary(b))) => {
                    let frame = Frame::decode(&b)?;
                    match frame {
                        Frame::HelloAck { .. } => break,
                        Frame::Error { message } => {
                            return Err(Error::Auth(message));
                        }
                        other => {
                            warn!(?other, "unexpected frame before hello_ack");
                        }
                    }
                }
                Some(Ok(Message::Close(_))) => {
                    return Err(Error::Network("closed before hello_ack".into()));
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(Error::WebSocket(e.to_string())),
                None => return Err(Error::Network("stream ended before hello_ack".into())),
            }
        }

        // Channel for outbound frames.
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Frame>();
        let peer_id = sync_handle.register_peer(out_tx.clone()).await?;

        // Send the initial sync message right away.
        if let Some(msg) = sync_handle.generate_sync_message(peer_id).await? {
            let _ = out_tx.send(Frame::Sync { bytes: msg });
        }

        // Writer task: pump frames out. Selects on a oneshot close signal so
        // graceful disconnect can interrupt the loop and emit a Close frame
        // before the underlying socket goes away.
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
                                // Peer is gone — skip the Close attempt.
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

        // Reader task: process inbound frames.
        let sync_for_reader = sync_handle.clone();
        let out_for_reader = out_tx.clone();
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
        });

        // Doc-change notifier: when local doc changes, generate sync messages.
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
        Frame::Hello { .. } | Frame::HelloAck { .. } => {
            debug!("ignoring late hello frame");
        }
    }
}

#[allow(dead_code)]
type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
