use crate::auth::{verify_auth_token, VaultKey};
use crate::error::{Error, Result};
use crate::net::client::handle_inbound;
use crate::net::protocol::Frame;
use crate::vault::SyncHandle;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

/// `agentsync --listen` server. Accepts websocket connections from peers and
/// bridges each one to its own SyncState within the local vault.
pub struct Server {
    accept_task: Option<JoinHandle<()>>,
    shutdown_tx: broadcast::Sender<()>,
    peer_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
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

        let (shutdown_tx, _) = broadcast::channel::<()>(8);
        let peer_tasks: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));

        let accept_shutdown_tx = shutdown_tx.clone();
        let mut accept_shutdown = shutdown_tx.subscribe();
        let peer_tasks_for_accept = peer_tasks.clone();
        let accept = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = accept_shutdown.recv() => break,
                    res = listener.accept() => {
                        let (stream, peer_addr) = match res {
                            Ok(t) => t,
                            Err(e) => {
                                warn!(error=%e, "accept");
                                continue;
                            }
                        };
                        let vault_id = vault_id.clone();
                        let vault_key = vault_key;
                        let sync_handle = sync_handle.clone();
                        let peer_shutdown = accept_shutdown_tx.subscribe();
                        let task = tokio::spawn(async move {
                            if let Err(e) = handle_peer(
                                stream,
                                peer_addr,
                                vault_id,
                                vault_key,
                                sync_handle,
                                peer_shutdown,
                            )
                            .await
                            {
                                warn!(error=%e, "peer connection ended");
                            }
                        });
                        peer_tasks_for_accept.lock().await.push(task);
                    }
                }
            }
        });

        Ok(Server {
            accept_task: Some(accept),
            shutdown_tx,
            peer_tasks,
            bound_addr: bound,
        })
    }

    /// Stop accepting new peers and gracefully close every active peer
    /// connection (each writer sends a Close frame before the socket is torn
    /// down). Bounded by a short timeout so an unresponsive peer can't hang
    /// shutdown indefinitely.
    pub async fn shutdown(mut self) {
        let _ = self.shutdown_tx.send(());
        if let Some(h) = self.accept_task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), h).await;
        }
        let mut tasks = self.peer_tasks.lock().await;
        for h in tasks.drain(..) {
            let _ = tokio::time::timeout(Duration::from_secs(2), h).await;
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(());
        if let Some(h) = self.accept_task.take() {
            h.abort();
        }
        // Outstanding peer_tasks: signaled via broadcast above. We can't await
        // in Drop, so they'll race with runtime teardown. Callers that care
        // about graceful close should call `shutdown().await`.
    }
}

async fn handle_peer(
    stream: tokio::net::TcpStream,
    peer_addr: SocketAddr,
    vault_id: String,
    vault_key: VaultKey,
    sync_handle: Arc<dyn SyncHandle>,
    mut shutdown_rx: broadcast::Receiver<()>,
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
        loop {
            tokio::select! {
                biased;
                _ = shutdown_rx.recv() => break,
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

    // The writer task is the primary lifecycle driver: it exits on graceful
    // shutdown, on a peer-side close, or on a write error. When it does, tear
    // down the others. The reader will exit naturally once writer closes its
    // side of the websocket (TCP EOF), so we wait briefly for it; notif has
    // a forever loop so we abort it.
    let _ = writer_task.await;
    notif_task.abort();
    let _ = tokio::time::timeout(Duration::from_millis(500), reader_task).await;
    // Idempotent — reader_task may have already done this on a peer-initiated
    // close. Calling it here too covers the shutdown-aborted-reader case.
    sync_handle.unregister_peer(peer_id).await;
    debug!(peer_id, "peer handler exited");
    Ok(())
}
