use crate::cli::{HubArgs, HubOp};
use crate::config::{read_or_default, write};
use agentsync_core::Pubkey;
use anyhow::Result;

pub async fn run(args: HubArgs) -> Result<()> {
    match args.op {
        HubOp::Trust { pubkey, path } => {
            // Validate before writing — refuse to silently store garbage.
            let pk = Pubkey::from_ssh_string(&pubkey).map_err(|e| anyhow::anyhow!(e))?;
            let canon = path.canonicalize().unwrap_or(path);
            let mut cfg = read_or_default(&canon)?;
            cfg.vault.hub_pubkey = Some(pk.to_ssh_string());
            write(&canon, &cfg)?;
            println!("pinned hub identity {} ({})", pk.to_ssh_string(), pk.fingerprint_sha256());
        }
        HubOp::Forget { path } => {
            let canon = path.canonicalize().unwrap_or(path);
            let mut cfg = read_or_default(&canon)?;
            cfg.vault.hub_pubkey = None;
            write(&canon, &cfg)?;
            println!("hub_pubkey cleared");
        }
        HubOp::Show { path } => {
            let canon = path.canonicalize().unwrap_or(path);
            let cfg = read_or_default(&canon)?;
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
