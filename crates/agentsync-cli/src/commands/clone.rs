use crate::cli::CloneArgs;
use crate::config::{
    identity_path, write, ConfigFile, IdentitySection, SyncSection, VaultSection,
};
use agentsync_core::net::client::ClientConn;
use agentsync_core::{Identity, OpenOptions, Pubkey, Vault};
use anyhow::{Context, Result};

pub async fn run(args: CloneArgs) -> Result<()> {
    let target = args.local_path.clone();
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

    // Pre-validate the --accept-hub-key value if any, so we fail before
    // spinning up a connection on a typo.
    let pre_accepted: Option<Pubkey> = match &args.accept_hub_key {
        Some(s) => Some(Pubkey::from_ssh_string(s).map_err(|e| anyhow::anyhow!(e))?),
        None => None,
    };

    let mut cfg = ConfigFile {
        vault: VaultSection {
            id: None,
            rendezvous_url: Some(args.rendezvous.clone()),
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

    let id_path = identity_path(&target, &cfg);
    let identity = if id_path.exists() {
        Identity::load_from_file(&id_path).map_err(|e| anyhow::anyhow!(e))?
    } else {
        let fresh = Identity::generate();
        fresh.save_to_file(&id_path).map_err(|e| anyhow::anyhow!(e))?;
        fresh
    };

    println!(
        "Local identity pubkey: {}",
        identity.pubkey().to_ssh_string()
    );
    println!(
        "(this device must be authorized in the vault's peers.md before clone can proceed)"
    );

    // Open the vault locally, then run the actual connect through ClientConn
    // so we can inspect the hub's pubkey before persisting trust.
    let vault_id_pre = args.vault_id.clone();
    let bind_opts = cfg.sync.to_bind_options();

    // Probe the hub: do the handshake, see who answered, decide whether to
    // accept. This uses ClientConn::connect with a temporary sync handle so
    // we don't spin up the full Vault until trust is settled.
    let probe_handle: std::sync::Arc<dyn agentsync_core::SyncHandle> =
        std::sync::Arc::new(NoopSyncHandle::default());
    let conn = ClientConn::connect(
        &args.rendezvous,
        vault_id_pre.clone(),
        pre_accepted,
        identity.clone(),
        probe_handle,
    )
    .await
    .with_context(|| format!("connect to rendezvous {}", args.rendezvous))?;
    let hub_pubkey = conn.hub_pubkey;
    let vault_id = conn.vault_id.clone();
    conn.close().await;

    // Decide whether to trust this hub. If pre_accepted matched we already
    // know; otherwise this is a TOFU prompt.
    let accepted_hub = if pre_accepted.is_some() {
        pre_accepted.unwrap()
    } else {
        prompt_trust(&args.rendezvous, &hub_pubkey)?;
        hub_pubkey
    };

    cfg.vault.id = Some(vault_id.clone());
    cfg.vault.hub_pubkey = Some(accepted_hub.to_ssh_string());
    write(&target, &cfg)?;
    println!("vault_id: {}", vault_id);
    println!("pinned hub: {}", accepted_hub.fingerprint_sha256());

    let opts = OpenOptions {
        rendezvous_url: Some(args.rendezvous.clone()),
        vault_id,
        identity,
        storage_path: storage,
        hub_pubkey: Some(accepted_hub),
    };
    let mut vault = Vault::open(opts).await?;
    let _binding = vault.bind_directory(&target, bind_opts).await?;
    if let Err(e) = vault.connect().await {
        anyhow::bail!("connect to rendezvous failed: {}", e);
    }
    println!("connected to {}", args.rendezvous);
    println!("syncing into {} — Ctrl-C to stop.", target.display());
    tokio::signal::ctrl_c().await?;
    vault.flush().await?;
    Ok(())
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
