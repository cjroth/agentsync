use crate::cli::WatchArgs;
use crate::commands::require_config;
use crate::config;
use agentsync_core::{OpenOptions, ReconnectOptions, Vault};
use anyhow::{Context, Result};
use tracing::info;

pub async fn run(args: WatchArgs) -> Result<()> {
    let path = match &args.path {
        Some(p) => p.clone(),
        None => std::env::current_dir()?,
    };
    let path = path.canonicalize().unwrap_or(path);
    let mut cfg = require_config(&path)?;
    let vault_id = cfg
        .vault
        .id
        .clone()
        .context(".agentsync/config.toml: vault.id missing")?;
    // CLI flags override config — let the user pick agent backend at runtime
    // without persisting it.
    if let Some(p) = &args.identity_agent {
        cfg.identity.agent_socket = Some(p.to_string_lossy().into_owned());
    }
    if let Some(s) = &args.identity_agent_pubkey {
        cfg.identity.agent_pubkey = Some(load_pubkey_arg(s)?);
    }
    let identity = config::resolve_identity(&path, &cfg)?;
    let storage = path.join(".agentsync");

    // Resolve rendezvous URL: --rendezvous flag wins, else config.toml,
    // else nothing. --offline forces nothing.
    let rendezvous_url = if args.offline {
        None
    } else {
        args.rendezvous
            .clone()
            .or_else(|| cfg.vault.rendezvous_url.clone())
    };

    let opts = OpenOptions {
        rendezvous_url: rendezvous_url.clone(),
        vault_id,
        identity,
        storage_path: storage,
        hub_pubkey: config::resolve_hub_pubkey(&cfg)?,
    };
    let mut vault = Vault::open(opts).await?;

    let bind_opts = cfg.sync.to_bind_options();
    let _binding = vault.bind_directory(&path, bind_opts).await?;

    if let Some(addr) = &args.listen {
        let parsed: std::net::SocketAddr = addr
            .parse()
            .with_context(|| format!("invalid --listen address: {}", addr))?;
        let bound = vault.listen(parsed).await?;
        info!(addr = %bound, "listening for peers");
        println!("listening on wss://{}", bound);
    } else if let Some(url) = &rendezvous_url {
        // Hand off to the supervisor: it does the initial connect with
        // backoff and reconnects automatically if the rendezvous goes away.
        // Returns immediately after spawning, so the watch loop can keep
        // running while the connection stabilizes in the background.
        vault
            .connect_with_reconnect(ReconnectOptions::default())
            .await?;
        println!("connecting to rendezvous: {}", url);
    }

    println!("watching {}", path.display());
    println!("vault_id: {}", vault.id());
    println!("identity_pub: {}", vault.pubkey().to_ssh_string());

    // Keep running until SIGINT.
    tokio::signal::ctrl_c().await?;
    println!("shutting down");
    vault.disconnect().await;
    vault.unlisten().await;
    vault.flush().await?;
    Ok(())
}

/// Parse an `--identity-agent-pubkey` argument: either a literal
/// `ssh-ed25519 <base64>` string, or a path to a file containing one
/// (matching the `<identity>.pub` sidecar `agentsync key generate` writes).
fn load_pubkey_arg(s: &str) -> Result<String> {
    if s.starts_with("ssh-") {
        return Ok(s.to_string());
    }
    let bytes = std::fs::read_to_string(s)
        .with_context(|| format!("read pubkey file at {}", s))?;
    let line = bytes
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty pubkey file at {}", s))?;
    Ok(line.trim().to_string())
}
