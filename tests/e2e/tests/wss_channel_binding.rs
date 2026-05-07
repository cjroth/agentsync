//! Phase 2 — WSS transport plus channel-bound application-layer auth.
//!
//! Pins the security claim of channel binding: an active MITM that
//! terminates TLS in the middle and re-encrypts to the real listener is
//! detected, because the fingerprint signed in the handshake transcript no
//! longer matches the cert the client actually saw.

use agentsync_core::tls::{client_config_accept_any, generate_self_signed, server_config};
use agentsync_core::Identity;
use agentsync_e2e::E2EVault;
use rustls_pki_types::ServerName;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// A relay that terminates TLS with its own cert, then re-encrypts to the
/// real hub. The relay does not understand websocket framing — it just pumps
/// decrypted bytes back and forth between the two TLS sessions.
async fn spawn_relay(real_hub_addr: SocketAddr) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = listener.local_addr().unwrap();

    // Generate a fresh cert for the relay — guaranteed different from the
    // real hub's cert.
    let (cert_der, key_der) = generate_self_signed().unwrap();
    let acceptor = TlsAcceptor::from(server_config(cert_der, key_der).unwrap());

    tokio::spawn(async move {
        loop {
            let (client_tcp, _) = match listener.accept().await {
                Ok(t) => t,
                Err(_) => continue,
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                // Terminate TLS with the relay's cert.
                let mut client_tls = match acceptor.accept(client_tcp).await {
                    Ok(s) => s,
                    Err(_) => return,
                };
                // Open a fresh TLS connection to the real hub.
                let hub_tcp = match TcpStream::connect(real_hub_addr).await {
                    Ok(t) => t,
                    Err(_) => return,
                };
                let connector = TlsConnector::from(client_config_accept_any());
                let server_name = ServerName::IpAddress(
                    real_hub_addr.ip().into(),
                );
                let mut hub_tls = match connector.connect(server_name, hub_tcp).await {
                    Ok(s) => s,
                    Err(_) => return,
                };
                // Pump bytes both directions until either side closes.
                let _ = tokio::io::copy_bidirectional(&mut client_tls, &mut hub_tls).await;
            });
        }
    });

    local
}

#[tokio::test]
async fn active_mitm_relay_fails_handshake() {
    let mut v = E2EVault::new().await.unwrap();

    // Pull the real hub's TCP address out of the harness URL.
    let real_addr: SocketAddr = v
        .rendezvous_url
        .strip_prefix("wss://")
        .unwrap()
        .parse()
        .unwrap();

    // Stand up a relay in front of the hub. It terminates TLS with its own
    // cert and re-encrypts to the real hub.
    let relay_addr = spawn_relay(real_addr).await;
    let relay_url = format!("wss://{}", relay_addr);

    // Authorize a client identity on the hub. This verifies that even with
    // valid auth credentials, the channel-binding check still rejects the
    // relay.
    let identity = Identity::generate();
    v.authorize_peer("legitimate", &identity.pubkey()).await.unwrap();

    // Try to discover the vault_id through the relay.
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        agentsync_core::net::client::discover_vault_id(&relay_url, &identity),
    )
    .await
    .expect("handshake hung — should have failed promptly");
    assert!(
        res.is_err(),
        "MITM relay was not detected; got Ok({:?})",
        res
    );

    let err = res.unwrap_err().to_string();
    assert!(
        err.to_lowercase().contains("fingerprint") || err.contains("auth"),
        "expected channel-binding rejection, got: {}",
        err
    );

    v.shutdown().await;
    let _ = Arc::new(()); // keep the rustls/Arc imports used
}

/// Restarting the listener with the same persisted cert keeps the cert
/// stable: the on-disk `tls.crt` is reused, not regenerated. Otherwise
/// every restart would force every peer to re-pin the hub.
#[tokio::test]
async fn cert_persists_across_listener_restart() {
    let mut v = E2EVault::new().await.unwrap();
    let cert_path = v
        .rendezvous
        .path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(".agentsync-server")
        .join("tls.crt");
    // The cert lives next to the rendezvous's storage dir.
    let storage = v.rendezvous.path().join(".agentsync");
    let tls_dir = storage.parent().unwrap().join(".agentsync-server");
    let crt = tls_dir.join("tls.crt");
    assert!(crt.exists(), "tls.crt should exist after first listen");
    let before = std::fs::read(&crt).unwrap();

    v.kill_rendezvous().await.unwrap();
    v.restart_rendezvous().await.unwrap();

    let after = std::fs::read(&crt).unwrap();
    assert_eq!(before, after, "tls.crt was regenerated across restart");
    let _ = cert_path; // silence unused
}
