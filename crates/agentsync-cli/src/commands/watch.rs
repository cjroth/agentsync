use crate::cli::WatchArgs;
use crate::commands::require_config;
use crate::config;
use agentsync_core::{OpenOptions, Vault};
use anyhow::{Context, Result};
use tracing::info;

pub async fn run(args: WatchArgs) -> Result<()> {
    let path = match &args.path {
        Some(p) => p.clone(),
        None => std::env::current_dir()?,
    };
    let path = path.canonicalize().unwrap_or(path);
    let cfg = require_config(&path)?;
    let vault_id = cfg
        .vault
        .id
        .clone()
        .context(".agentsync/config.toml: vault.id missing")?;
    let key = config::resolve_key(&cfg, None)?;
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
        vault_key: key,
        storage_path: storage,
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
        println!("listening on ws://{}", bound);
    } else if let Some(url) = &rendezvous_url {
        match vault.connect().await {
            Ok(_) => println!("connected to rendezvous: {}", url),
            Err(e) => tracing::warn!(error = %e, url, "could not connect to rendezvous"),
        }
    }

    println!("watching {}", path.display());
    println!("vault_id: {}", vault.id());

    // Keep running until SIGINT.
    tokio::signal::ctrl_c().await?;
    println!("shutting down");
    vault.disconnect().await;
    vault.unlisten().await;
    vault.flush().await?;
    Ok(())
}
