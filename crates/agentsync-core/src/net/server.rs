use crate::auth::{build_transcript, random_nonce, NONCE_LEN};
use crate::error::{Error, Result};
use crate::identity::{Identity, Pubkey};
use crate::net::client::handle_inbound;
use crate::net::protocol::Frame;
use crate::peers_md::AuthorizedPeer;
use crate::tls::{cert_fingerprint, server_config};
use crate::vault::SyncHandle;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tracing::{debug, info, warn};

type AcceptedStream = TlsStream<tokio::net::TcpStream>;

/// `agentsync --listen` server. Accepts websocket connections from peers and
/// bridges each one to its own SyncState within the local vault.
pub struct Server {
    accept_task: Option<JoinHandle<()>>,
    enforcer_task: Option<JoinHandle<()>>,
    shutdown_tx: broadcast::Sender<()>,
    peer_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    pub bound_addr: SocketAddr,
}

impl Server {
    /// Bind the listener with a self-signed TLS cert. The cert is loaded
    /// from `<storage_path>/../.agentsync-server/` if present, otherwise a
    /// fresh one is generated and persisted there.
    pub async fn bind(
        addr: SocketAddr,
        vault_id: String,
        vault_name: Option<String>,
        identity: Identity,
        sync_handle: Arc<dyn SyncHandle>,
        cert_der: Vec<u8>,
        key_der: Vec<u8>,
    ) -> Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        info!(addr = %bound, vault_id, "rendezvous listening");

        let server_tls = server_config(cert_der.clone(), key_der)?;
        let acceptor = TlsAcceptor::from(server_tls);
        let cert_fp = cert_fingerprint(&cert_der);

        let (shutdown_tx, _) = broadcast::channel::<()>(8);
        let peer_tasks: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));

        let accept_shutdown_tx = shutdown_tx.clone();
        let mut accept_shutdown = shutdown_tx.subscribe();
        let peer_tasks_for_accept = peer_tasks.clone();
        let identity_for_accept = identity.clone();
        let sync_handle_for_accept = sync_handle.clone();
        let vault_id_for_accept = vault_id.clone();
        let vault_name_for_accept = vault_name.clone();
        let acceptor_for_accept = acceptor.clone();
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
                        let vault_id = vault_id_for_accept.clone();
                        let vault_name = vault_name_for_accept.clone();
                        let identity = identity_for_accept.clone();
                        let sync_handle = sync_handle_for_accept.clone();
                        let acceptor = acceptor_for_accept.clone();
                        let peer_shutdown = accept_shutdown_tx.subscribe();
                        let task = tokio::spawn(async move {
                            let tls_stream = match acceptor.accept(stream).await {
                                Ok(s) => s,
                                Err(e) => {
                                    warn!(peer=%peer_addr, error=%e, "tls accept");
                                    return;
                                }
                            };
                            if let Err(e) = handle_peer(
                                tls_stream,
                                peer_addr,
                                vault_id,
                                vault_name,
                                identity,
                                sync_handle,
                                cert_fp,
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

        let mut enforcer_shutdown = shutdown_tx.subscribe();
        let enforcer_handle = sync_handle.clone();
        let enforcer = tokio::spawn(async move {
            // Seed with the initial set so we don't spam "peer added" for
            // every entry on startup.
            let mut known: HashMap<Pubkey, String> = enforcer_handle
                .authorized_peers()
                .await
                .into_iter()
                .map(|p| (p.pubkey, p.label))
                .collect();
            loop {
                tokio::select! {
                    biased;
                    _ = enforcer_shutdown.recv() => break,
                    _ = enforcer_handle.wait_doc_changed() => {
                        let current = enforcer_handle.authorized_peers().await;
                        log_authorized_diff(&known, &current);
                        let pubkeys: Vec<Pubkey> = current.iter().map(|p| p.pubkey).collect();
                        enforcer_handle.disconnect_unauthorized_peers(&pubkeys).await;
                        known = current.into_iter().map(|p| (p.pubkey, p.label)).collect();
                    }
                }
            }
        });

        Ok(Server {
            accept_task: Some(accept),
            enforcer_task: Some(enforcer),
            shutdown_tx,
            peer_tasks,
            bound_addr: bound,
        })
    }

    pub async fn shutdown(mut self) {
        let _ = self.shutdown_tx.send(());
        if let Some(h) = self.accept_task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), h).await;
        }
        if let Some(h) = self.enforcer_task.take() {
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
        if let Some(h) = self.enforcer_task.take() {
            h.abort();
        }
    }
}

/// Compare the previously-known authorized set against the current parse of
/// `authorized_keys` and emit a single `info!` line per addition or removal.
/// The enforcer runs on every doc-change notification (any synced file edit),
/// so the common case is "no change" — short-circuit before allocating.
fn log_authorized_diff(prev: &HashMap<Pubkey, String>, current: &[AuthorizedPeer]) {
    if prev.len() == current.len() && current.iter().all(|p| prev.contains_key(&p.pubkey)) {
        return;
    }
    for p in current {
        if !prev.contains_key(&p.pubkey) {
            info!(
                fp = %p.pubkey.fingerprint_sha256(),
                label = %label_or_unlabeled(&p.label),
                "peer added to authorized_keys",
            );
        }
    }
    use std::collections::HashSet;
    let cur_keys: HashSet<Pubkey> = current.iter().map(|p| p.pubkey).collect();
    for (pk, label) in prev {
        if !cur_keys.contains(pk) {
            info!(
                fp = %pk.fingerprint_sha256(),
                label = %label_or_unlabeled(label),
                "peer removed from authorized_keys",
            );
        }
    }
}

fn label_or_unlabeled(s: &str) -> &str {
    if s.is_empty() {
        "(unlabeled)"
    } else {
        s
    }
}

async fn handle_peer(
    stream: AcceptedStream,
    peer_addr: SocketAddr,
    vault_id: String,
    vault_name: Option<String>,
    identity: Identity,
    sync_handle: Arc<dyn SyncHandle>,
    cert_fp: [u8; 32],
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    let ws = accept_async(stream).await?;
    let (mut writer, mut reader) = ws.split();

    let hub_pubkey = identity.pubkey();
    let hub_nonce = random_nonce();
    // Channel binding: cover the SHA-256 of the listener's TLS cert so a
    // relayed MITM cannot substitute its own TLS endpoint.
    let tls_fp: Vec<u8> = cert_fp.to_vec();
    let hello_hub = Frame::HelloHub {
        vault_id: vault_id.clone(),
        hub_identity_pubkey: hub_pubkey.as_bytes().to_vec(),
        hub_nonce: hub_nonce.to_vec(),
        tls_cert_fingerprint: tls_fp.clone(),
        vault_name,
    };
    writer.send(Message::binary(hello_hub.encode()?)).await?;

    let frame = read_one_frame(&mut reader).await?;
    let (peer_pubkey_bytes, peer_nonce_bytes, _op) = match frame {
        Frame::HelloPeer {
            peer_identity_pubkey,
            peer_nonce,
            op,
        } => (peer_identity_pubkey, peer_nonce, op),
        Frame::Error { message } => {
            return Err(Error::Protocol(format!(
                "peer reported error: {}",
                message
            )));
        }
        _ => return Err(Error::Protocol("expected HelloPeer".into())),
    };
    let peer_pubkey = Pubkey::from_bytes(&peer_pubkey_bytes)?;
    if peer_nonce_bytes.len() != NONCE_LEN {
        return Err(Error::Protocol("peer nonce wrong length".into()));
    }
    let mut peer_nonce = [0u8; NONCE_LEN];
    peer_nonce.copy_from_slice(&peer_nonce_bytes);

    let authorized = sync_handle.authorized_pubkeys().await;
    if !authorized.contains(&peer_pubkey) {
        let _ = writer
            .send(Message::binary(
                Frame::Error {
                    message: format!(
                        "peer pubkey {} not authorized",
                        peer_pubkey.fingerprint_sha256()
                    ),
                }
                .encode()?,
            ))
            .await;
        return Err(Error::Auth(format!(
            "peer not in authorized_keys: {}",
            peer_pubkey.fingerprint_sha256()
        )));
    }

    let transcript = build_transcript(
        &hub_nonce,
        &peer_nonce,
        &tls_fp,
        hub_pubkey.as_bytes(),
        peer_pubkey.as_bytes(),
    );
    let hub_sig = identity.sign(&transcript).await?;
    let proof_hub = Frame::ProofHub {
        sig: hub_sig.to_vec(),
    };
    writer.send(Message::binary(proof_hub.encode()?)).await?;

    let frame = read_one_frame(&mut reader).await?;
    let peer_sig = match frame {
        Frame::ProofPeer { sig } => sig,
        Frame::Error { message } => {
            return Err(Error::Protocol(format!(
                "peer reported error: {}",
                message
            )));
        }
        _ => return Err(Error::Protocol("expected ProofPeer".into())),
    };
    if !peer_pubkey.verify(&transcript, &peer_sig) {
        let _ = writer
            .send(Message::binary(
                Frame::Error {
                    message: "peer signature failed verification".into(),
                }
                .encode()?,
            ))
            .await;
        return Err(Error::Auth("peer signature failed verification".into()));
    }
    info!(peer=%peer_addr, fp=%peer_pubkey.fingerprint_sha256(), "peer authenticated");

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Frame>();
    let peer_id = sync_handle
        .register_peer(out_tx.clone(), Some(peer_pubkey))
        .await?;

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

    let _ = writer_task.await;
    notif_task.abort();
    let _ = tokio::time::timeout(Duration::from_millis(500), reader_task).await;
    sync_handle.unregister_peer(peer_id).await;
    debug!(peer_id, "peer handler exited");
    Ok(())
}

async fn read_one_frame<S>(
    reader: &mut futures_util::stream::SplitStream<WebSocketStream<S>>,
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
