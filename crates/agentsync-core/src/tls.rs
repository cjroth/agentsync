//! Self-signed TLS for agentsync's WSS transport (Phase 2 of AUTH.md).
//!
//! The listener generates a self-signed ed25519 cert at first run and
//! persists it next to the vault storage. Clients accept *any* cert at the
//! TLS layer — trust comes from the application-layer signature in the
//! handshake, which binds to the cert fingerprint and so defeats relayed
//! MITM.

use crate::error::{Error, Result};
use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, ServerConfig, SignatureScheme};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const TLS_DIR: &str = ".agentsync-server";
pub const CERT_FILE: &str = "tls.crt";
pub const KEY_FILE: &str = "tls.key";

/// Path conventions: certs land in `<storage_path>/../.agentsync-server/`,
/// i.e. one level above the vault's `.agentsync/` so they're not synced.
pub fn tls_dir_for_storage(storage_path: &Path) -> PathBuf {
    let parent = storage_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    parent.join(TLS_DIR)
}

/// Load a persisted cert/key pair, or generate + persist a fresh ed25519
/// self-signed cert with a 10-year lifetime.
pub fn load_or_generate_self_signed(dir: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    let crt_path = dir.join(CERT_FILE);
    let key_path = dir.join(KEY_FILE);
    if crt_path.exists() && key_path.exists() {
        let cert_der = std::fs::read(&crt_path)?;
        let key_der = std::fs::read(&key_path)?;
        return Ok((cert_der, key_der));
    }
    let (cert_der, key_der) = generate_self_signed()?;
    std::fs::create_dir_all(dir)?;
    write_with_mode(&key_path, &key_der, 0o600)?;
    std::fs::write(&crt_path, &cert_der)?;
    Ok((cert_der, key_der))
}

/// Build a fresh ed25519 self-signed cert. Returns DER-encoded (cert, key).
pub fn generate_self_signed() -> Result<(Vec<u8>, Vec<u8>)> {
    let mut params = CertificateParams::new(vec!["agentsync.local".to_string()])
        .map_err(|e| Error::Other(format!("rcgen params: {}", e)))?;
    params.distinguished_name = DistinguishedName::new();
    let now = std::time::SystemTime::now();
    params.not_before = now.into();
    params.not_after = (now + std::time::Duration::from_secs(10 * 365 * 24 * 3600)).into();
    let key_pair = KeyPair::generate_for(&rcgen::PKCS_ED25519)
        .map_err(|e| Error::Other(format!("rcgen keypair: {}", e)))?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| Error::Other(format!("rcgen sign: {}", e)))?;
    let cert_der = cert.der().to_vec();
    let key_der = key_pair.serialize_der();
    Ok((cert_der, key_der))
}

/// SHA-256 of the cert DER. This is the value bound into the handshake
/// transcript — both peers commit to having seen the same TLS endpoint.
pub fn cert_fingerprint(cert_der: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(cert_der);
    let out = hasher.finalize();
    let mut fp = [0u8; 32];
    fp.copy_from_slice(&out);
    fp
}

/// Server-side rustls config built from a DER cert + DER key.
pub fn server_config(cert_der: Vec<u8>, key_der: Vec<u8>) -> Result<Arc<ServerConfig>> {
    install_default_provider();
    let cert_chain = vec![CertificateDer::from(cert_der)];
    let key = PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(key_der));
    let cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .map_err(|e| Error::Other(format!("rustls server cert: {}", e)))?;
    Ok(Arc::new(cfg))
}

/// Client-side rustls config that accepts *any* cert. The trust decision is
/// deferred to the application-layer handshake.
pub fn client_config_accept_any() -> Arc<ClientConfig> {
    install_default_provider();
    let cfg = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAny))
        .with_no_client_auth();
    Arc::new(cfg)
}

fn install_default_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // Idempotent: if another consumer beat us to it, ignore the result.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// A TLS verifier that approves any certificate. Safe in this codebase
/// because the application-layer signature commits both parties to the cert
/// fingerprint they actually saw — relaying or substitution is detected
/// after the TLS handshake completes.
#[derive(Debug)]
struct AcceptAny;

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ED25519,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
        ]
    }
}

#[cfg(unix)]
fn write_with_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)?;
    use std::io::Write;
    f.write_all(bytes)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_with_mode(path: &Path, bytes: &[u8], _mode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_round_trips() {
        let (cert, key) = generate_self_signed().unwrap();
        assert!(!cert.is_empty());
        assert!(!key.is_empty());
        let fp = cert_fingerprint(&cert);
        assert_eq!(fp.len(), 32);
        // Same cert always produces the same fingerprint.
        assert_eq!(fp, cert_fingerprint(&cert));
    }

    #[test]
    fn fingerprint_changes_per_cert() {
        let (a, _) = generate_self_signed().unwrap();
        let (b, _) = generate_self_signed().unwrap();
        assert_ne!(cert_fingerprint(&a), cert_fingerprint(&b));
    }
}
