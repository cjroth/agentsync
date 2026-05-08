use crate::cli::CloneArgs;
use crate::config::{
    identity_path, write, ConfigFile, IdentitySection, SyncSection, VaultSection,
};
use agentsync_core::net::client::ClientConn;
use agentsync_core::{normalize_rendezvous_url, Identity, OpenOptions, Pubkey, Vault};
use anyhow::{Context, Result};
use std::path::PathBuf;

pub async fn run(args: CloneArgs) -> Result<()> {
    let rendezvous = normalize_rendezvous_url(&args.remote_url);

    // Pre-validate the --accept-hub-key value if any, so we fail before
    // spinning up a connection on a typo.
    let pre_accepted: Option<Pubkey> = match &args.accept_hub_key {
        Some(s) => Some(Pubkey::from_ssh_string(s).map_err(|e| anyhow::anyhow!(e))?),
        None => None,
    };

    // Identity for the probe lives at the user-default path even before we
    // know where to put the local clone — the identity isn't tied to the
    // clone directory.
    let probe_identity_path = args
        .identity
        .clone()
        .unwrap_or_else(crate::config::user_identity_default);
    let identity = if probe_identity_path.exists() {
        Identity::load_from_file(&probe_identity_path).map_err(|e| anyhow::anyhow!(e))?
    } else {
        let fresh = Identity::generate();
        fresh
            .save_to_file(&probe_identity_path)
            .map_err(|e| anyhow::anyhow!(e))?;
        fresh
    };

    println!(
        "Local identity pubkey: {}",
        identity.pubkey().to_ssh_string()
    );
    println!(
        "(this device must be authorized in the vault's authorized_keys before clone can proceed)"
    );

    // Probe the hub: do the handshake, see who answered, decide whether to
    // accept. This uses ClientConn::connect with a temporary sync handle so
    // we don't spin up the full Vault until trust is settled.
    let vault_id_pre = args.vault_id.clone();
    let probe_handle: std::sync::Arc<dyn agentsync_core::SyncHandle> =
        std::sync::Arc::new(NoopSyncHandle::default());
    let conn = ClientConn::connect(
        &rendezvous,
        vault_id_pre.clone(),
        pre_accepted,
        identity.clone(),
        probe_handle,
    )
    .await
    .with_context(|| format!("connect to rendezvous {}", rendezvous))?;
    let hub_pubkey = conn.hub_pubkey;
    let vault_id = conn.vault_id.clone();
    let remote_name = conn.vault_name.clone();
    conn.close().await;

    // Resolve the target directory: explicit arg wins, else the remote
    // vault's name, else the URL host.
    let target = match args.local_path.clone() {
        Some(p) => p,
        None => PathBuf::from(default_target_dir(&rendezvous, remote_name.as_deref())?),
    };
    if target.exists() {
        let has_other = std::fs::read_dir(&target)?
            .filter_map(|e| e.ok())
            .any(|e| e.file_name() != ".agentsync");
        if has_other {
            anyhow::bail!("clone target {} is not empty", target.display());
        }
    }
    std::fs::create_dir_all(&target)?;
    let target = target.canonicalize()?;
    let storage = target.join(".agentsync");

    let mut cfg = ConfigFile {
        vault: VaultSection {
            id: None,
            name: remote_name.clone(),
            rendezvous_url: Some(rendezvous.clone()),
            hub_pubkey: pre_accepted.as_ref().map(|p| p.to_ssh_string()),
        },
        identity: IdentitySection {
            path: args
                .identity
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            agent_socket: None,
            agent_pubkey: None,
        },
        sync: SyncSection::default(),
    };

    let bind_opts = cfg.sync.to_bind_options();
    // The identity may have landed at a different path (e.g. user default)
    // than what the freshly-written config will resolve later — make sure
    // the file is also accessible from the per-vault default location.
    let id_path = identity_path(&target, &cfg);
    if !id_path.exists() {
        identity
            .save_to_file(&id_path)
            .map_err(|e| anyhow::anyhow!(e))?;
    }

    // Decide whether to trust this hub. If pre_accepted matched we already
    // know; otherwise this is a TOFU prompt.
    let accepted_hub = if pre_accepted.is_some() {
        pre_accepted.unwrap()
    } else {
        prompt_trust(&rendezvous, &hub_pubkey)?;
        hub_pubkey
    };

    cfg.vault.id = Some(vault_id.clone());
    cfg.vault.hub_pubkey = Some(accepted_hub.to_ssh_string());
    write(&target, &cfg)?;
    println!("vault_id: {}", vault_id);
    println!("pinned hub: {}", accepted_hub.fingerprint_sha256());

    let opts = OpenOptions {
        rendezvous_url: Some(rendezvous.clone()),
        vault_id,
        identity,
        storage_path: storage,
        hub_pubkey: Some(accepted_hub),
        name: remote_name.clone(),
    };
    let mut vault = Vault::open(opts).await?;
    let _binding = vault.bind_directory(&target, bind_opts).await?;
    if let Err(e) = vault.connect().await {
        anyhow::bail!("connect to rendezvous failed: {}", e);
    }
    println!("connected to {}", rendezvous);
    println!("syncing into {} — Ctrl-C to stop.", target.display());
    tokio::signal::ctrl_c().await?;
    vault.flush().await?;
    Ok(())
}

/// Pick a default local directory for the clone, preferring the remote
/// vault's name and falling back to the URL host. Errors if neither is
/// available — the caller can then ask the user to pass an explicit dir.
fn default_target_dir(rendezvous: &str, remote_name: Option<&str>) -> Result<String> {
    if let Some(n) = remote_name {
        let trimmed = n.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    if let Ok(parsed) = url::Url::parse(rendezvous) {
        if let Some(host) = parsed.host_str() {
            if !host.is_empty() {
                return Ok(host.replace('.', "-"));
            }
        }
    }
    anyhow::bail!(
        "remote vault has no name and URL host is unavailable; \
         pass an explicit local directory: agentsync clone <url> <dir>"
    )
}

/// Prompt the user to confirm a freshly-seen hub identity. Returns Ok(())
/// on yes; bails otherwise. Reads `[y/N]` from stdin.
fn prompt_trust(url: &str, hub_pubkey: &Pubkey) -> Result<()> {
    use std::io::{BufRead, Write};
    eprintln!();
    eprintln!("The hub at {} has identity:", url);
    eprintln!("  {}", hub_pubkey.to_ssh_string());
    eprintln!("  {}", hub_pubkey.fingerprint_sha256());
    eprintln!();
    eprintln!("This is the first time connecting. Trust this hub? [y/N]");
    eprint!("> ");
    let _ = std::io::stderr().flush();
    let stdin = std::io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    if matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        anyhow::bail!("hub identity rejected; aborting clone");
    }
}

/// Sync handle that does nothing — used for the trust-probing connection
/// before we know whether to accept the hub. Once trust is decided, the
/// caller drops this connection and opens a real Vault.
#[derive(Default)]
struct NoopSyncHandle;

#[async_trait::async_trait]
impl agentsync_core::SyncHandle for NoopSyncHandle {
    async fn register_peer(
        &self,
        _out: tokio::sync::mpsc::UnboundedSender<agentsync_core::Frame>,
        _pubkey: Option<agentsync_core::Pubkey>,
    ) -> agentsync_core::Result<u64> {
        Ok(0)
    }
    async fn unregister_peer(&self, _peer_id: u64) {}
    async fn generate_sync_message(
        &self,
        _peer_id: u64,
    ) -> agentsync_core::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    async fn receive_sync_message(
        &self,
        _peer_id: u64,
        _bytes: &[u8],
    ) -> agentsync_core::Result<()> {
        Ok(())
    }
    async fn read_blob(&self, _hash: &str) -> agentsync_core::Result<Vec<u8>> {
        Err(agentsync_core::Error::NotFound("noop".into()))
    }
    async fn write_blob(&self, _hash: &str, _bytes: &[u8]) -> agentsync_core::Result<()> {
        Ok(())
    }
    async fn wait_doc_changed(&self) {
        std::future::pending::<()>().await;
    }
    async fn authorized_pubkeys(&self) -> Vec<agentsync_core::Pubkey> {
        Vec::new()
    }
    async fn disconnect_unauthorized_peers(&self, _authorized: &[agentsync_core::Pubkey]) {}
}
