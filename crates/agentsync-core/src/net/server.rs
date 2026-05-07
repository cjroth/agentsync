use crate::auth::{verify_auth_token, VaultKey};
use crate::error::{Error, Result};
use crate::net::client::handle_inbound;
use crate::net::protocol::Frame;
use crate::vault::SyncHandle;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

/// `agentsync --listen` server. Accepts websocket connections from peers and
/// bridges each one to its own SyncState within the local vault.
pub struct Server {
    pub _accept: JoinHandle<()>,
    pub bound_addr: SocketAddr,
}

impl Server {
    pub async fn bind(
        addr: SocketAddr,
        vault_id: String,
        vault_key: VaultKey,
        sync_handle: Arc<dyn SyncHandle>,
    ) -> Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        info!(addr = %bound, vault_id, "rendezvous listening");

        let accept = tokio::spawn(async move {
            loop {
                let (stream, peer_addr) = match listener.accept().await {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(error=%e, "accept");
                        continue;
                    }
                };
                let vault_id = vault_id.clone();
                let vault_key = vault_key;
                let sync_handle = sync_handle.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        handle_peer(stream, peer_addr, vault_id, vault_key, sync_handle).await
                    {
                        warn!(error=%e, "peer connection ended");
                    }
                });
            }
        });

        Ok(Server {
            _accept: accept,
            bound_addr: bound,
        })
    }
}

async fn handle_peer(
    stream: tokio::net::TcpStream,
    peer_addr: SocketAddr,
    vault_id: String,
    vault_key: VaultKey,
    sync_handle: Arc<dyn SyncHandle>,
) -> Result<()> {
    let ws = accept_async(stream).await?;
    let (mut writer, mut reader) = ws.split();

    // Receive HELLO.
    let hello_bytes = match reader.next().await {
        Some(Ok(Message::Binary(b))) => b,
        _ => {
            return Err(Error::Protocol("missing hello".into()));
        }
    };
    let hello = Frame::decode(&hello_bytes)?;
    let (their_vault, their_token, _op) = match hello {
        Frame::Hello {
            vault_id: vid,
            auth_token,
            op,
        } => (vid, auth_token, op),
        _ => return Err(Error::Protocol("first frame must be hello".into())),
    };
    // If the client volunteered a vault_id, it must match. If they didn't
    // (fresh-clone path), we'll just tell them what ours is in HelloAck.
    if let Some(vid) = &their_vault {
        if vid != &vault_id {
            let err = Frame::Error {
                message: format!("vault mismatch: expected {}", vault_id),
            };
            let _ = writer.send(Message::binary(err.encode()?)).await;
            return Err(Error::Auth("vault id mismatch".into()));
        }
    }
    if !verify_auth_token(&vault_key, &their_token) {
        let err = Frame::Error {
            message: "auth token rejected".into(),
        };
        let _ = writer.send(Message::binary(err.encode()?)).await;
        return Err(Error::Auth("token mismatch".into()));
    }
    let ack = Frame::HelloAck {
        vault_id: vault_id.clone(),
    };
    writer.send(Message::binary(ack.encode()?)).await?;
    info!(peer=%peer_addr, "peer authenticated");

    // Register the peer with the sync hub.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Frame>();
    let peer_id = sync_handle.register_peer(out_tx.clone()).await?;

    // Initial sync message.
    if let Some(msg) = sync_handle.generate_sync_message(peer_id).await? {
        let _ = out_tx.send(Frame::Sync { bytes: msg });
    }

    let writer_task = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            let bytes = match frame.encode() {
                Ok(b) => b,
                Err(e) => {
                    warn!(error=%e, "encode frame");
                    continue;
                }
            };
            if writer.send(Message::binary(bytes)).await.is_err() {
                break;
            }
        }
        let _ = writer.close().await;
    });

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
        debug!(peer_id, "peer disconnected");
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

    let _ = tokio::join!(writer_task, reader_task, notif_task);
    Ok(())
}
