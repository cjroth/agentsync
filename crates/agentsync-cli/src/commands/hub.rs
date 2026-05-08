use crate::cli::{HubArgs, HubOp};
use crate::config::{read_or_default, write};
use agentsync_core::Pubkey;
use anyhow::Result;
use std::path::PathBuf;

pub async fn run(cwd: PathBuf, args: HubArgs) -> Result<()> {
    let path = cwd.canonicalize().unwrap_or(cwd);
    match args.op {
        HubOp::Trust { pubkey } => {
            // Validate before writing — refuse to silently store garbage.
            let pk = Pubkey::from_ssh_string(&pubkey).map_err(|e| anyhow::anyhow!(e))?;
            let mut cfg = read_or_default(&path)?;
            cfg.vault.hub_pubkey = Some(pk.to_ssh_string());
            write(&path, &cfg)?;
            println!("pinned hub identity {} ({})", pk.to_ssh_string(), pk.fingerprint_sha256());
        }
        HubOp::Forget => {
            let mut cfg = read_or_default(&path)?;
            cfg.vault.hub_pubkey = None;
            write(&path, &cfg)?;
            println!("hub_pubkey cleared");
        }
        HubOp::Show => {
            let cfg = read_or_default(&path)?;
            match cfg.vault.hub_pubkey.as_deref() {
                Some(s) => match Pubkey::from_ssh_string(s) {
                    Ok(pk) => println!("{} ({})", pk.to_ssh_string(), pk.fingerprint_sha256()),
                    Err(_) => println!("{}", s),
                },
                None => println!("(no hub_pubkey pinned)"),
            }
        }
    }
    Ok(())
}
