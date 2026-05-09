//! Native crypto: OsRng, file-backed Identity Signer, rcgen-based TLS.

use crate::error::Result;
use crate::host::crypto::{Rng, Signer, TlsCert, TlsCertProvider};
use crate::identity::{Identity, Pubkey, SIGNATURE_LEN};
use crate::tls;
use async_trait::async_trait;
use rand_core::{OsRng, RngCore};
use std::path::Path;

pub struct OsRngProvider;

impl Rng for OsRngProvider {
    fn fill_bytes(&self, buf: &mut [u8]) {
        OsRng.fill_bytes(buf);
    }
}

/// Adapts a [`crate::Identity`] (file or ssh-agent) into the [`Signer`]
/// trait. The async signature on `Identity::sign` matches the trait
/// directly.
pub struct IdentitySigner {
    inner: Identity,
}

impl IdentitySigner {
    pub fn new(inner: Identity) -> Self {
        Self { inner }
    }
}

#[async_trait(?Send)]
impl Signer for IdentitySigner {
    async fn sign(&self, msg: &[u8]) -> Result<[u8; SIGNATURE_LEN]> {
        self.inner.sign(msg).await
    }

    fn pubkey(&self) -> Pubkey {
        self.inner.pubkey()
    }
}

pub struct NativeTlsProvider;

#[async_trait(?Send)]
impl TlsCertProvider for NativeTlsProvider {
    async fn load_or_generate(&self, dir: &Path) -> Result<TlsCert> {
        let (cert_der, key_der) = tls::load_or_generate_self_signed(dir)?;
        Ok(TlsCert { cert_der, key_der })
    }
}
